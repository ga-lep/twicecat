# Phase 9 Video Plan
Status: APPROVED (2026-10-13): Q6-Q8 answered by the human (Vulkan Video for Intel, AMD and NVIDIA before Ada; NVENC/NVDEC through CUDA on Ada and newer; no VA-API), the other questions with their recommended answers (R), including the Q2 version bump and the Q11 crates (rule 3). Work happens on branch `feat/video`, cut from `fix/pre-video` (the two pre-phase fixes, waiting for review) so that Task 1 can re-run their key-logging test. Tracked in [ga-lep/twicecat#22](https://github.com/ga-lep/twicecat/issues/22): a task is ticked there when its commit lands; notes go to PROGRESS.md and `phase9-video.progress.md`.

**Amended 2026-10-14 (human), after Task 5 and the encoder spike below: LiveKit's own encoders, no fork.** The video is encoded by the hardware encoders already inside webrtc-sys (NVENC, VideoToolbox) or by libwebrtc's software VP8; their simulcast comes from libwebrtc. We keep capture, colour conversion and rendering. The pre-encoded passthrough, the Q5 fork and our own encoders are dropped: Task 4's Vulkan Video encoder is removed (its Vulkan device, DMA-BUF import and compute conversion stay, for capture), Task 7 is dropped, Task 6 and Task 15 are rewritten. Windows ships software encoding for now. Bugs found upstream are reported to livekit/rust-sdks (the human posts the drafts); their fixes arrive through a version bump, asked for first (rule 3). **Later, not in this phase's gates (human, 2026-10-14):** hardware video on Intel and AMD under Linux (1080p60 and 4K30, once LiveKit's VA-API crash is fixed, possibly by our own upstream PR) and on Windows (Media Foundation, upstream #1282). They are expected to be merged and supported eventually, so the encoder choice (Task 6) keeps a slot for them and turns them on when a release has them. The superseded answers are kept below, struck through in place, for the record.

**Before this phase (fixes outside this plan, on branch `fix/pre-video`: `fix(core): play a screen share's sound beside the sharer's voice`, `fix(core): keep call keys out of the log whatever RUST_LOG asks`):**
- Remote audio players are keyed by participant identity only (`crates/core/src/call/media.rs:602-625`), so an Element Call user's screen-share audio replaces their microphone, is voice-cleaned, and stopping the share silences them. Fix: key players by (identity, source).
- The native frame cryptor logs key material at INFO (webrtc-sdk/webrtc#308) and we set no libwebrtc log routing: check whether encrypted audio calls already leak keys into logs, and fix it if so.

## Tasks checklist
- [ ] Task 1: Bump livekit and check the logs for key material (phase9.1, `feat(core)`)
- [ ] Task 2: `twicecat-video` skeleton, frames, test pattern, ladder, API shape (phase9.2, `feat(video)` + core API)
- [ ] Task 3: Hardware probe and backend measurement spike (phase9.3, `feat(video)`)
- [ ] Task 4: Vulkan compute and Vulkan Video H.264 encoder (phase9.4, `feat(video)`)
- [ ] Task 5: Publishing and receiving in the core, software path (phase9.5, `feat(core)`)
- [ ] Task 6: Hardware encoding through LiveKit's encoders (phase9.6, `feat(core)` + `feat(video)` + `dev`)
- [x] ~~Task 7: NVENC encoder through CUDA~~ dropped 2026-10-14 (LiveKit's NVENC replaces it)
- [ ] Task 8: Linux screen sharing (phase9.8, `feat(video)` + core)
- [ ] Task 9: Linux camera (phase9.9, `feat(video)` + core)
- [ ] Task 10: App: video renderer and call stage (phase9.10, `feat(app)`)
- [ ] Task 11: App: controls, pickers, preview, Video settings (phase9.11, `feat(app)`)
- [ ] Task 12: Windows and macOS (phase9.12, `feat(video)` + core)
- [ ] Task 13: Steam Frame (phase9.13, `feat(video)`; conditional)
- [ ] Task 14: 4K60 acceptance and tuning (phase9.14, `feat(video)` + docs)
- [ ] Task 15: Hardware decode checks and upstream fixes (phase9.15, `feat(core)`; gated on LiveKit releases)
- [ ] Task 16: Wrap-up (phase9.16, `docs`)

## Goal
Camera video and screen sharing (with its audio) in MatrixRTC calls, up to 3840×2160 at 60 fps in both directions, interoperable with Element Call in encrypted rooms, on Linux (Wayland and X11), Windows, macOS, and later the Steam Frame.

**How we get there (amended 2026-10-14): we own capture, conversion and rendering in Rust; libwebrtc encodes, decodes and carries the video.** Capture and colour conversion are our code (`crates/video`); raw I420/NV12 frames go into LiveKit's `NativeVideoSource`. libwebrtc (through `livekit`) encodes them with the hardware encoders built into webrtc-sys (NVENC on NVIDIA under Linux, VideoToolbox on macOS) or its software VP8, makes the simulcast layers (`SimulcastEncoderAdapter`), and does what it already does for audio: congestion control, packetisation, retransmission, jitter buffers, SRTP and the E2EE frame cryptor. It decodes with NVDEC where the CUDA build has it, else in software. ~~Encoded frames enter it through LiveKit's pre-encoded ("passthrough") video source; our own hardware decoders once LiveKit exposes encoded frames on the receive side.~~ (superseded)

**Rust-native rule for this phase.** Everything we write is Rust. FFI is allowed only to OS and driver APIs (Vulkan, the NVIDIA driver's CUDA, NVENC and NVDEC, Media Foundation, Direct3D, VideoToolbox, ScreenCaptureKit, AVFoundation, PipeWire, V4L2) and to libwebrtc, which Phase 6 already uses. No FFmpeg, no GStreamer, no OpenH264. Every `unsafe` site follows the Phase 6 decision 7 rule (SAFETY comment, listed in PROGRESS.md). A new crate is added only when writing it ourselves would not be justified, and each one is in the Q11 list for approval.

Rules for every task (AGENTS.md):
- **Rule 2:** every `livekit`, `libwebrtc`, `ash`, NVIDIA driver (CUDA driver API, NVENC, NVDEC: the Video Codec SDK headers of the version the probe requires), `ashpd`, `pipewire`, `windows`, `objc2-*` and `iced` API is read in the registry source of the pinned version before use and cited file:line in PROGRESS.md. The facts below come from planning and are re-checked by each task.
- **Rule 3:** the version bumps in Q2 and the crates in Q11 need the human's approval first. Nothing else changes.
- **Rule 5:** one `feat(<scope>)` commit per task with a `Step: phase9.<n>` trailer (scopes `video`, `core`, `app`, `dev`). Review fixes are folded in with `--fixup` and `rebase --autosquash`. PROGRESS.md is updated in the same commit, and the task is ticked in this issue.
- **Rule 6:** tests use the local stack only (Synapse plus both local SFUs, `scripts/stack.sh`) and the test accounts. Frames, window titles, device serials, encoded bytes and keys are never logged.
- **Rule 7:** the app talks to the core only through `Command`s, `CoreEvent`s and the new SDK-free `VideoFeed` handle in `crates/core/src/api/`. `scripts/check-app-deps.sh` and `scripts/check-core-api.sh` gain `twicecat-video`, `ash` and the capture crates.
- **Rule 8:** stop and ask if a LiveKit encoder or decoder bug blocks a task's acceptance and no setting avoids it (we don't patch webrtc-sys), or if a hardware path misses its budget on the human's machine after tuning.
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

## Encoder spike (2026-10-14): LiveKit's built-in encoders on the human's laptop
Pinned livekit 0.9.5 / libwebrtc 0.3.52 / webrtc-sys 0.3.49, built with `CUDA_HOME=/opt/cuda` (Arch `cuda` 13.4.2, installed by the human; only `cuda.h` is used at build time, `libcuda`/`libnvcuvid` are loaded at run time). The Task 5 test pattern was published alice → carol on the local stack, with `TrackPublishOptions { video_codec, video_encoder, simulcast, video_encoding }` switched from the environment (spike code, not committed). Numbers are from `RtcStats` outbound/inbound.

| Encoder | Case | Result |
|---|---|---|
| NVENC H.264, 3 layers | 4K30, encrypted | works: 29 fps received at 3840×2160, top layer about 12.8 ms per frame (including the I420 upload), NVDEC decode on the receiver |
| NVENC H.264, 3 layers | 1080p60, encrypted | works: 60 fps sent, 54.7 fps received |
| NVENC H.264 | 4K60 | **fails**: `nvEncInitializeEncoder` error 8. The factory advertises only `42e01f` (`W/src/nvidia/nvidia_encoder_factory.cpp:229-233`) and the encoder pins NVENC to that level 3.1 (`W/src/nvidia/h264_encoder_impl.cpp:93-105`) |
| NVENC AV1, 1 layer | 4K60, unencrypted | works: 60.4 fps received, 8.9 ms encode, dav1d software decode 5.2 ms |
| NVENC AV1 | 4K60, encrypted | receiver decodes nothing (48 keyframes in 30 s); livekit-client can't decrypt AV1 either |
| NVENC H.265, 3 layers | 4K60 | sends at 60 fps, but NVDEC H.265 on the receiver stays at its first size (320×180) |
| VA-API H.264 (Arrow Lake iGPU, iHD 26.3.5) | every size, 1 or 3 layers | **SIGSEGV** inside `iHD_drv_video.so` (`vaRenderPicture`) on the first frame; ffmpeg's `h264_vaapi` on the same device with the same settings works, so LiveKit's wrapper (`W/src/vaapi/vaapi_h264_encoder_wrapper.cpp`, identical on upstream `main`) triggers it |

- Backends: `TrackPublishOptions.video_encoder: VideoEncoderBackend` (`livekit-0.9.5/src/room/options.rs:137`), `VideoEncoderBackend::list_available()` (`L/src/rtp_sender.rs:37-57`); an unavailable backend falls back to another encoder with a warning. `Auto` would pick VA-API on an Intel or AMD machine, so we never ask for `Auto` or `Hardware`.
- Windows: webrtc-sys builds no hardware encoder (`W/build.rs:138-172`); upstream PR livekit/rust-sdks#1282 (open) adds Media Foundation H.264.
- LiveKit's preset for a 4K source is 30 fps (`H2160`); 60 fps needs an explicit `video_encoding.max_framerate`.
- A process that exits while an NVDEC decoder is still alive hangs in `libnvcuvid`'s exit handler (stack in the drafts); the VP8 path on the same build exits normally. The core must close calls before the process ends.
- CPU (sender and receiver in one process, test pattern included): 55-65 % of one core with NVENC H.264 at 4K30.

**Capture crates checked and rejected (2026-10-14):** `scap` 0.1.0-beta.1 (MIT) doesn't compile on Linux, uses SHM only (two CPU copies per frame), has no restore token, no Flatpak (`OpenPipeWireRemote` unused) and no X11; `xcap` 0.9.8 (Apache-2.0) is screenshot-first, its Wayland recording is unfinished and SHM only; `nokhwa` 0.10.11 (Apache-2.0) opens `/dev/video*` directly (no Camera portal, no Flatpak), has open bugs in its Media Foundation and AVFoundation paths, and its 0.11 rewrite doesn't stream yet. Capture stays ours on `ashpd`/`pipewire` as Q9 and Q10 say.

**Upstream reports** (drafted 2026-10-14, posted by the human): NVENC H.264 level pinned to 3.1; VA-API segfault; AV1 with E2EE not decoded; NVDEC H.265 stuck at its first size; exit hang with a live NVDEC decoder. Related and already upstream: #1492 (H.264 profiles offered beyond `42e01f`, Firefox gets no video), #1499 (lazy NVDEC CUDA context), #1500 (VA-API lifecycle, in 0.3.49).

## Questions
**Q1. Numbering, order and branch.** (R) Phase 9, after the Phase 6 call UI checkpoint; independent of Phase 8 (spaces). Branch `feat/video`. Trailers `Step: phase9.<n>`.

**Q2. Version bump (rule 3).** (R) Bump `livekit` 0.9.3 → 0.9.5 (libwebrtc 0.3.52, webrtc-sys 0.3.49) in Task 1, for the passthrough frame-drop fix and `set_bitrate`, with the prebuilt libwebrtc tag and SHA-256 in `scripts/fetch-libwebrtc.sh` updated if the new `webrtc-sys-build` names a new tag. The whole audio suite and the call stack tests must pass unchanged before any video work.

**Q3. Who encodes.** **Amended 2026-10-14 (human): (b′) libwebrtc encodes with the hardware encoders already in webrtc-sys, no fork** (spike above).
- (a) ~~(R)~~ (superseded) We do: our encoders feed the passthrough source. libwebrtc's own encoders are used only as the no-hardware fallback (raw I420 into `NativeVideoSource`, VP8, at most 1080p30).
- (b) libwebrtc encodes; we fork webrtc-sys to add hardware encoders in C++.
- (c) Replace libwebrtc with a Rust WebRTC stack (str0m) and drive LiveKit's signalling ourselves. Most control, but it rewrites Phase 6's transport, E2EE and audio path (rule 8). Deferred; revisit after this phase.

**Q4. Codecs.** Element Call's browsers cannot decrypt AV1 and get no fallback codec under E2EE. **Amended 2026-10-14:** H.264 (Constrained Baseline, `42e01f`) when NVENC or VideoToolbox encodes, VP8 on the software path; no AV1 opt-in for now (it isn't decoded in encrypted calls, natively or in the browser), no H.265 (Firefox, and the NVDEC resize bug).
- (a) (R) **H.264 everywhere by default**, Constrained Baseline with levels up to 5.2 (4K60), 4-byte start codes, every hardware encoder supports it, every browser decodes it. **AV1 as an opt-in** "Better quality (twicecat and unencrypted calls only)" setting, on encoders that have it: NVENC on Ada and newer (Task 7) first; Vulkan Video AV1 needs an `ash` release with `VK_KHR_video_encode_av1` (the locked 0.38.0 has none), asked for when one exists. No VP9 (no NVIDIA/AMD/Apple hardware encode), no H.265 (Firefox, no software decoder in our prebuilt).
- (b) H.264 High profile by default (about 10-15 % fewer bits), after checking that Firefox and Chrome decode High in a stream negotiated as `42e01f`.

**Q5. Simulcast with our encoder.** **Resolved 2026-10-14:** with LiveKit's encoders the layers come from libwebrtc's `SimulcastEncoderAdapter` (`W/src/video_encoder_factory.cpp:686-701`), on NVENC too (three NVIDIA H.264 encoders in the spike); no passthrough, no fork. The original question, superseded: the passthrough is single-layer, and the SFU pauses a layer a viewer cannot take, so one 4K layer leaves viewers on slow links with nothing.
- (a) (R) We extend the passthrough upstream (livekit/rust-sdks): accept one encoded stream per simulcast layer (index 0-2), and on the receive side adopt PR #1515. Until a release ships it, we carry the change as a pinned git fork through `[patch.crates-io]` (webrtc-sys bridge code only; the prebuilt libwebrtc is unchanged). Each layer's fork commit is reviewed like our own code.
- (b) Single layer only, with our encoder following the bitrate target (everyone gets the same picture; slow viewers stall).
- (c) Publish separate tracks per layer (Element Call would show extra screen shares; rejected).

**Q6. Hardware encode backends.** **Amended 2026-10-14 (human):** what webrtc-sys has, chosen per machine by the core:
- **NVIDIA on Linux, every generation with the proprietary driver:** LiveKit's NVENC (`video_encoder: Nvenc`), H.264 with three layers; the build needs `cuda.h` (Task 6), the libraries are loaded at run time, so the same binary runs without NVIDIA.
- **Intel and AMD on Linux:** software VP8 until LiveKit's VA-API crash is fixed (we ask for `Software` explicitly so `Auto` never picks VA-API).
- **macOS:** LiveKit's VideoToolbox. **Windows:** software VP8; Media Foundation hardware when upstream #1282 is released (version bump, asked first). **Steam Frame:** software.
- Hybrid laptops: NVENC uses CUDA device 0 (`W/src/nvidia/cuda_context.cpp:162`); frames reach it from CPU memory (`ToI420()`, `W/src/nvidia/h264_encoder_impl.cpp:289`), so capture never has to cross GPUs.

The superseded answer of 2026-10-13: Vulkan for most GPUs, CUDA for modern NVIDIA; no VA-API.
- **NVIDIA Ada and newer** (RTX 40 and 50, compute capability ≥ 8.9; the human's RTX 5080): **NVENC** through the NVIDIA driver's CUDA driver API, on Linux and Windows. H.264 and AV1 (Q4 opt-in) encode.
- **Intel, AMD and NVIDIA before Ada:** **Vulkan Video** (`ash`, already in the lock), on Linux and Windows.
- **Shared by both:** capture import (DMA-BUF, D3D11), colour conversion and scaling run in our own Vulkan compute shaders. NVENC takes the result through Vulkan→CUDA external memory (opaque fd / Win32 handle), checked in Task 3.
- **Fallbacks:** an NVIDIA GPU before Ada whose driver has no Vulkan Video encode uses NVENC. On Windows, Media Foundation comes next. Then the software path (Q3). macOS uses **VideoToolbox**, the Steam Frame **V4L2 stateful**.
- **Choice per machine:** a probe makes it at run time (vendor, compute capability through `cuDeviceGetAttribute`, NVENC caps, Vulkan Video caps), and settings can override it.
- **To check in Task 3:** NVENC, NVDEC and the CUDA driver API come with the NVIDIA driver (`libcuda`, `libnvidia-encode`, `libnvcuvid`; `nvcuda.dll`, `nvEncodeAPI64.dll`, `nvcuvid.dll`), not with the CUDA runtime that `scripts/fetch-cuda-runtime.sh` downloads. If so, they work in the light build. NVENC on Linux needs the proprietary driver: with nouveau or NVK the probe offers Vulkan Video or software. GeForce drivers cap concurrent NVENC sessions. Up to 3 layers × camera and screen means 6 sessions, and Task 3 records the cap.
- Task 3 measures NVENC on the RTX 5080 and Vulkan Video on the Arrow Lake iGPU (and on the RTX 5080, for comparison) before the backend tasks start. No AMD GPU or NVIDIA GPU before Ada is on record: those paths are human checkpoint items.

**Q7. Decoding.** **Amended 2026-10-14:** libwebrtc decodes; NVDEC H.264 comes with the CUDA build, the rest is software. Our own decoders are dropped. The superseded answer: (R) Phase 9 renders what libwebrtc decodes (software H.264 via FFmpeg, AV1 via dav1d, VP8/VP9 via libvpx). Answered by the human (2026-10-13), with the Q6 split. Task 15 adds our own hardware decode: **NVDEC** through CUDA on Ada and newer, **Vulkan Video** decode on Intel, AMD and NVIDIA before Ada, Media Foundation as the Windows fallback, VideoToolbox on macOS through the receive-side passthrough once it is released or carried by the Q5 fork. 4K60 software H.264 decode is measured in Task 14 so we know the CPU cost until then.

**Q8. Getting frames to the screen.** Answered by the human (2026-10-13), with the Q6 split. The core hands the app a `VideoFeed` per track: a latest-frame slot (NV12 or I420 planes in `Arc<[u8]>`, size, colour info, timestamp) with a wake-up the app's subscription waits on at most once per display frame. The app draws it with an iced `shader` widget: one persistent wgpu texture per plane, `queue.write_texture` per frame, YUV→RGB (BT.709, limited/full range) in WGSL. No `image::Handle` per frame (it allocates a new texture each time and splits 4K over 4 atlas layers). A Vulkan Video or NVDEC decoder (Task 15) copies its NV12 output to host memory for the `VideoFeed` (NVDEC: `cuMemcpy2D` from the mapped frame). Zero-copy from decoder to screen (CUDA→Vulkan external memory, then a wgpu import) waits for an iced release on wgpu ≥30 (a later pin bump).

**Q9. Screen picker and capture per platform.** (R)
- Wayland: the xdg-desktop-portal ScreenCast picker (`ashpd` `screencast`) with PipeWire DMA-BUF streams (our `pipewire` client); cursor embedded; "remember this choice" stores the portal restore token, opt-in.
- X11: our own picker (screens and windows with thumbnails) and XShm capture through `x11rb`; 4K60 on X11 is best effort.
- Windows: our own picker; `Windows.Graphics.Capture` through `windows` (D3D11 textures).
- macOS: the system picker (`SCContentSharingPicker`) and ScreenCaptureKit (IOSurface frames).

**Q10. Camera capture.** (R) Linux: the Camera portal (`ashpd` `camera`) and PipeWire, so it works inside Flatpak; NV12/YUYV directly, MJPEG decoded by `zune-jpeg` with two frames in flight (4K60 needs it). Windows: Media Foundation `IMFSourceReader` (its MJPEG decoder). macOS: AVFoundation. Cameras that offer only 4K30 are offered at 4K30.

**Q11. New crates and features (rule 3).** (R) Newest releases at the start of each task, exact pins:
- `yuvutils-rs` (CPU colour conversion and scaling; pure Rust SIMD) - Task 2;
- no crate for NVENC, NVDEC or CUDA: ~~our own bindings~~ LiveKit's (amended 2026-10-14); `libloading` stays for the Task 3 probe; `cuda.h` at build time only (Task 6, not a crate);
- `objc2-video-toolbox`, `objc2-core-video`, `objc2-core-media`, `objc2-screen-capture-kit`, `objc2-av-foundation` (macOS) - Task 12;
- ~~`v4l2r` (Steam Frame encoder) - Task 13~~ dropped 2026-10-14;
- `naga` 27.0.3 (already locked through wgpu) as a build-dependency of `twicecat-video`: our compute shaders are WGSL, compiled to SPIR-V and validated in `build.rs` (human, 2026-10-13, Task 4);
- features on existing pins: `ashpd` `screencast`, `camera`, `pipewire`; `windows` Media Foundation, Direct3D 11, Graphics.Capture, WinRT features; `ash` (already locked) as a direct dependency; `x11rb` 0.13.2 and `zune-jpeg` 0.5.15 as direct dependencies at their locked versions.

**Q12. Quality presets and limits.** **Amended 2026-10-14:** what each encoder can do today: NVENC up to 4K30 and 1440p60/1080p60 (4K60 after the level fix upstream), software up to 1080p30 (Task 5). The rest stands. (R) A per-share choice: Auto, 720p30, 1080p60, 1440p60, 4K30, 4K60 (default Auto = the highest the encoder probe and the measured uplink allow, capped at 1440p60 until the human raises it). Screen shares keep resolution and drop framerate first; cameras keep framerate and drop resolution first. Starting bitrates for H.264 (tuned in Task 14): 4K60 20 Mbps (screen) and 25 Mbps (camera), 1440p60 12 Mbps, 1080p60 8 Mbps; lower layers 1080p30 4 Mbps and 540p15 0.8 Mbps (screen), 720p30 1.7 Mbps and 360p30 0.45 Mbps (camera). AV1 at 0.6 × these.

## Architecture
**New crate `crates/video` (`twicecat-video`)**, like `crates/audio`: no Matrix, no LiveKit, no UI.
- `frame`: `Frame` = CPU planes (NV12, I420, YUYV, BGRA with strides) or a GPU handle (`DmaBuf { planes, modifier }`, D3D11 shared handle, IOSurface), plus size, colour space, timestamp.
- `capture`: `ScreenCapturer` and `CameraCapturer` traits; backends `portal` (Wayland), `x11`, `wgc` (Windows), `sck` (macOS), `pw_camera`, `mf_camera`, `avf_camera`; `TestPattern` (moving content plus a machine-readable frame counter and send timestamp) for tests.
- `gpu`: our own Vulkan device (`ash`), DMA-BUF and D3D11 import, compute shaders for BGRA→I420/NV12, read back to CPU memory for `NativeVideoSource`.
- ~~`encode`: `Encoder` trait and our backends~~ (dropped 2026-10-14). The probe keeps reporting the GPUs; the core combines it with `VideoEncoderBackend::list_available()` to pick the encoder.
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

Acceptance (amended by the human 2026-10-13, after Task 3 found no usable Vulkan encoder on the Arrow Lake iGPU and Q6 was kept): encode tests on the RTX 5080 through Vulkan Video, forced over NVENC, as the path that NVIDIA GPUs before Ada take (NVENC is its default there, Task 7); on the iGPU, the probe sends it to the software path, and the compute conversion, scaling and DMA-BUF import run there. AMD and NVIDIA before Ada stay on the human checkpoint. Encode: 4K60 test pattern for 60 s, all three layers, encode p99 under 8 ms, bitrate within ±15 % of target; the stream is written to a file and checked with `ffprobe -count_frames` when that tool is installed (test-only, skipped otherwise; the end-to-end decode check is in Task 6); unit tests on the CPU-side logic. Result accepted by the human 2026-10-13: on the RTX 5080's Vulkan path p99 is 7.9-12 ms (under the 16.7 ms 4K60 frame budget) and the 540p15 layer runs +19% (the driver's encoder floors at its top QP on the test pattern); both go to Task 14.

### Task 5: Publishing and receiving in the core, software path (phase9.5, `feat(core)`)
- Video tracks in `call/video.rs`; libwebrtc-encoded fallback (raw path, VP8 ≤1080p30); remote subscription, `VideoFeed`, adaptive stream, dynacast, `SetVideoView` (sizes in physical pixels, since the SFU picks layers by physical height); remote camera mute shows as `VideoTrackView.muted`; CLI `/video pattern <res> <fps>`, `/video stats`.
- Remote screen-share audio plays with its own volume (`SetUserVolume` gains a `source`), never cleaned, never a speaking indicator (builds on the pre-phase keying fix).

Acceptance (stack tests, CI): two CLIs in an **encrypted** room; alice publishes a 720p30 test pattern; bob's feed delivers it in order with the counter intact, ≥ 28 fps over 30 s; leaving the call stops the feeds; a `RemoteTrack::Video` from another device appears as a `VideoTrackView`.

### Task 6: Hardware encoding through LiveKit's encoders (phase9.6, `feat(core)` + `feat(video)` + `dev`)
Rewritten 2026-10-14 (the Q5 fork and the passthrough are dropped).
- **Build:** webrtc-sys compiles NVENC/NVDEC when `cuda.h` is found (`W/build.rs:281-313`). A `dev` script fetches the CUDA driver-API header only (no toolkit; NVIDIA's licence for it checked first, it is never shipped), with a pinned version and SHA-256 like `scripts/fetch-cuda-runtime.sh`, and `.cargo/config.toml` points `CUDA_HOME` at it; CI and release builds use it. The binary still starts on machines without NVIDIA (libraries loaded at run time).
- **Encoder choice:** NVENC with H.264 where the probe finds an NVIDIA GPU on the proprietary driver and `list_available()` lists `Nvenc`; VideoToolbox on macOS; otherwise `Software` with VP8, never `Auto` or `Hardware` (VA-API crash). A setting can force software.
- **Presets:** explicit `video_encoding` (bitrate and `max_framerate`) per Q12 quality, so 60 fps isn't lost to LiveKit's 30 fps 4K preset; caps per encoder (NVENC 4K30 and 1080p60, measured in the spike; 1440p60 not measured yet and dropped to 1080p60 if NVENC refuses it at level 3.1; software 1080p30); simulcast as LiveKit lays it out.
- **Remove Task 4's Vulkan Video encoder** (folded into the Task 4 commit with `--fixup`; the Vulkan device, DMA-BUF import and compute conversion stay, and the Task 4 tests that cover them).
- **Encoder loss mid-call:** if stats show the encoder fell back or stopped (`encoder_implementation`, frames not encoded), the core republishes on the software path and tells the app once, like the audio GPU fallback in Phase 6.
- **Clean exit:** `Shutdown` closes every call and waits for LiveKit's teardown before the core reports that it stopped (the NVDEC exit hang).
- `VideoStats` gains the encoder in use (`encoder_implementation`, hardware or not) and the decoder for received tracks; `VideoEncoders` reports the choice.

Acceptance:
- CI (no GPU): the software path is unchanged (Task 5 tests); the encoder choice is unit-tested for every probe result; a build with the fetched header compiles NVENC in.
- Hardware, on the human's machine (ignored tests, `hw-video`): encrypted call, test pattern, H.264 through NVENC with three layers: 4K30 ≥ 28 fps and 1440p60 and 1080p60 ≥ 55 fps delivered over 30 s, in order; a small tile gets a low layer and a full-screen tile the top one; the test process exits by itself after the call ends; shaping alice's uplink to 8 Mbps (a test-only `netem` sidecar with `NET_ADMIN` in `docker-compose.yml`, never `tc` on the host) lowers the top layer without freezes longer than 500 ms.

### ~~Task 7: NVENC encoder through CUDA~~ (dropped 2026-10-14)
LiveKit's NVENC encodes (Task 6). The original text is in git history.

### Task 8: Linux screen sharing (phase9.8, `feat(video)` + core)
- Portal ScreenCast with DMA-BUF and SHM fallback, restore tokens (opt-in), cursor embedded, stream end when the user stops it from the compositor; X11 picker data and XShm capture; screen-share audio through our PipeWire client, excluding our own call playback nodes.
- Amended 2026-10-14: DMA-BUF frames are imported and converted to I420 by Task 4's Vulkan compute on the GPU that produced them (matched by DRM render node), then read back for `NativeVideoSource`; SHM frames are converted on the CPU (`yuvutils-rs`).
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
- Camera and Share buttons, Wayland portal flow, X11/Windows picker grid, share-audio checkbox, quality picker (Q12), a "You are sharing" bar with Stop that stays visible above every overlay and a camera-on mark, self-view, Video card (device, quality, codec, encoder status, "Use hardware encoding" (amended 2026-10-14; no iGPU encoder until VA-API is fixed), upload cap), shortcuts (Ctrl+Shift+V camera, Ctrl+Shift+S share), `docs/accessibility.md`.

Acceptance: `update` tests for every control and error path (portal cancelled, device unplugged, encoder lost).

### Task 12: Windows and macOS (phase9.12, `feat(video)` + core)
- Windows (amended 2026-10-14): WGC capture with D3D11 import and our compute conversion, software VP8 up to 1080p30 until LiveKit ships a Windows hardware encoder (#1282), Media Foundation camera, WASAPI process-loopback share audio.
- macOS: ScreenCaptureKit with the system picker, LiveKit's VideoToolbox H.264 (HEVC off, Q4), AVFoundation camera, ScreenCaptureKit audio; `NSCameraUsageDescription` and screen-recording notes for Phase 7.

Acceptance: CI builds and unit tests on both OSes; hardware tests are human items until a machine is on record (rule 9).

### Task 13: Steam Frame (phase9.13, `feat(video)`; conditional)
- Software encoding (amended 2026-10-14; no `v4l2r`), gamescope/KDE capture through the portal; measured CPU and battery cost. Runs only when the human has the device available.

### Task 14: 4K60 acceptance and tuning (phase9.14, `feat(video)` + docs)
- The full-path hardware suite: encrypted room, 120 s, 4K60 screen share and 4K60 camera (test pattern), measuring delivered fps, freezes, capture-to-feed latency from the embedded timestamps (the last hop to the screen is a human item), encode and decode time, CPU and GPU load, uplink bitrate. Default bitrates and presets tuned from the results.
- A/V offset: the test pattern flashes white with a 1 kHz tone every 2 s; the receiver measures the offset between the flash in the feed and the tone in the mixed output.
- `packet_buffer_size_video` in `dev/livekit/livekit.yaml` raised to what the measurements need, and the value documented for server admins in `docs/building.md`.

Amended 2026-10-14: the 4K60 gate applies once LiveKit's NVENC can encode 4K60 (the level fix); until then it runs at 4K30 and 1440p60.

Acceptance on the human's machine, per available backend: delivered ≥ 58 fps average (≥ 28 at 4K30), no freeze over 250 ms, p95 capture-to-feed latency under 120 ms on the local stack, encode p99 under 10 ms, A/V offset within ±45 ms; the results table in PROGRESS.md. Decoding is still software here (Q7): if software 4K60 H.264 decode cannot hold 58 fps, the shortfall is recorded and the receive-side fps gate moves to Task 15.

### Task 15: Hardware decode checks and upstream fixes (phase9.15, `feat(core)`; gated on LiveKit releases)
Rewritten 2026-10-14: our own decoders are dropped.
- Measure NVDEC H.264 decode (from the CUDA build) and the software decoders on the receiver in the Task 14 suite.
- For each upstream report that has been fixed in a release (by LiveKit, or by our own upstream PR, which the human decides on first): ask for the version bump (rule 3), re-run the spike cases, and lift the matching limit (4K60 on NVENC, VA-API on Intel and AMD, AV1 in encrypted calls once livekit-client decrypts it too, Windows hardware encode).

Acceptance: the Task 14 suite with the receiver's decoder and CPU cost recorded; each lifted limit has its spike case passing.

### Task 16: Wrap-up (phase9.16, `docs`)
- Independent review of `main..HEAD` (privacy: frames, titles, keys; every `unsafe` listed), status of the upstream reports, the checkpoint list, ROADMAP status.

## Human checkpoint (never ticked by the agent; rule 9)
- [x] Plan questions Q1-Q12 answered and plan approved; crates in Q11 approved. Approved 2026-10-13 (Q6-Q8 as the human chose, the rest as recommended).
- [ ] Encrypted call with Element Call in Chrome and Firefox (and Safari if a Mac, iPhone or iPad is available): our 4K60 screen share and camera shown sharp, with sound, in spotlight; their VP8 camera and screen share, and their share audio, shown and heard here.
- [ ] Lip sync by eye and ear with a camera at 4K60 and 1080p30.
- [ ] Wayland portal picker on KDE and GNOME, restore token, stop from the compositor; X11 picker.
- [ ] A real 4K60 webcam (or the best camera on hand) at its top mode; hot-plug.
- [ ] Screen-share audio from a game and a video, without our own call audio in it.
- [ ] Colours (limited/full range), fullscreen and pop-out, HiDPI and mixed-scale monitors, the sharing bar always visible, light and dark themes, keyboard-only use.
- [ ] Unplugging power during a share on the hybrid laptop: the share keeps going (NVENC, or software at lower quality if the dGPU goes away).
- [ ] Windows and macOS: permissions, pickers, capture and encode on real hardware.
- [ ] An NVIDIA GPU before Ada encoding through LiveKit's NVENC; an AMD GPU and an NVIDIA GPU on nouveau or NVK on the software path, as the probe says.
- [ ] Element Call in Firefox shows our H.264 camera and screen share (upstream #1492: Constrained Baseline must be what is negotiated).
- [ ] Steam Frame, if Task 13 runs.

## Options considered and deferred
- **Replacing libwebrtc with str0m** (Q3 c): full control of transport and E2EE in Rust; revisit after Phase 9 with the measurements from Task 14.
- **Zero-copy decode to screen:** waits for an iced release on wgpu ≥30 and multi-plane DMA-BUF import in wgpu.
- **Camera H.264 passthrough** (UVC cameras that encode themselves): single layer only, revisit with Q5's layers.
- **AV1 by default:** only once livekit-client can decrypt AV1.
- **Our own encoders through the passthrough** (Vulkan Video, NVENC bindings; the original Q3 a, Q5 a, Tasks 4, 6, 7): dropped 2026-10-14 for LiveKit's encoders. Revisit only if the upstream fixes don't land and 4K60 or Intel/AMD hardware encoding becomes a requirement.
- **Capture crates** (`scap`, `xcap`, `nokhwa`): rejected 2026-10-14, see the spike section.
</content>
</invoke>
