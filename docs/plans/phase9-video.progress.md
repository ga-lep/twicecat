Plan: drafted 2026-10-13 for ga-lep/twicecat#22; Q6-Q8 answered by the human and the other questions approved as recommended, 2026-10-13
Task 1: started
Task 1: complete (commit "feat(core): move to livekit 0.9.5 for the passthrough encoder fixes", gate ok: 1700 tests; ignored 156 passed; GPU audio 9, core 10 passed)
  notes: [prebuilt] same tag webrtc-89d790b, no fetch change; [keys] raw libwebrtc log still has 6 key lines per call, 0 pass our filter; [upstream] trusted rate controller and keyframe-gated deltas in the passthrough, PeerConnection::set_bitrate
Task 2: started
Task 2: built (commit "feat(video): add the video crate and the core's video API", gate ok: 1732 tests; stack call tests pass)
Task 2: complete (API shape approved by the human 2026-10-13; Phase 9 goes on with the Phase 6 call UI checkpoint open, human 2026-10-13)
  notes: [api] CameraPreview(VideoFeed) added beside the plan's events; NotSupported("video") for every video command until its task; [intent] JoinCall intent now written to membership and ring, read from incoming rings; [stale] engine/calls.rs is never compiled
Task 3: started
Task 3: built (probe, video_probe example, scripts/measure-encoders.sh; measured on the human's machine)
  notes: [nvenc] 12 sessions, Vulkan->CUDA interop ok, H.264 4K60 at 307 fps, AV1 488 fps; [intel] ANV Vulkan encode hidden (ANV_DEBUG=video-encode), CQP only, level 5.1; VA-API 145 fps CBR +0.7%; [nvidia-vulkan] FFmpeg h264_vulkan: High only, deadlocks at the second IDR; [q6] to be revisited with the human
Task 3: complete (Q6 revisited with the human 2026-10-13 and kept: Vulkan Video only for Intel/AMD/pre-Ada NVIDIA, software otherwise, no VA-API)
Task 4: started
Task 4: complete (commit "feat(video): convert, scale and encode H.264 with Vulkan", gate ok: 1748 tests; hardware suite 6 passed, also under the validation layer)
  notes: [accepted] p99 7.9-12 ms (plan 8) and 540p15 +19% (plan 15%) on the RTX 5080's Vulkan path, accepted by the human 2026-10-13 for Task 14; [nvidia] rate control needs VkVideoEncodeH264RateControlLayerInfoKHR; LOW_LATENCY faster than ULTRA_LOW_LATENCY; Baseline works; [intel] readback memory must be the cached type; [dma-buf] dGPU VRAM buffers lose the iGPU device
Task 5: started
Task 5: complete (commit "feat(core): send and receive video in calls on the software path", gate ok: 1756 tests; ignored suite 140 passed)
  notes: [measured] 720p30 pattern in an encrypted call: 29.3 fps over 30 s, in order, every stamp read; adaptive stream starts at 320x180 until SetVideoView; [api] CoreConfig::video (VideoBackend::TestPattern), SetUserVolume source; [share-audio] the SFU never marks share sound as speaking; [open] received colour space not exposed by libwebrtc (BT.601 assumed), LiveKit's 720p default is 3 Mb/s
Task 6: started
Task 6: complete (commit "feat(core): encode video in hardware through LiveKit's encoders", gate ok: 1772 tests; ignored 139 passed, 1 one-off (call_media share sound, passed on rerun); hardware NVENC and fallback passed)
  notes: [nvenc] 4K30 30.1, 1440p60 60.1, 1080p60 60.0 fps in three layers, NVDEC on the receiver; 4K60 capped (LiveKit's H.264 level 3.1); [abort] frames stop 150 ms before unpublish (SimulcastEncoderAdapter deque abort, upstream draft 6); [exit] NVDEC alive at exit hangs (upstream draft 5); [flake] 720p30 VP8 test misses 28 fps about 1 run in 8, bursts of loss in the first seconds, not the bitrate (A/B 9/10 each), for Task 14; [build] cuda.h from the pinned CUDA runtime wheel, CUDA_HOME=dev/cuda
Task 7: dropped (human 2026-10-14: LiveKit's NVENC replaces it)
Plan: amended 2026-10-14 by the human: Task 11 gains advanced video settings for camera and screen (size, frame rate, bitrate, codec, encoder, layers/SVC, degradation preference), core included; Task 9 opens the camera mode the settings name
