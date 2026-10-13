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
