Plan: drafted 2026-10-13 for ga-lep/twicecat#22; Q6-Q8 answered by the human and the other questions approved as recommended, 2026-10-13
Task 1: started
Task 1: complete (commit "feat(core): move to livekit 0.9.5 for the passthrough encoder fixes", gate ok: 1700 tests; ignored 156 passed; GPU audio 9, core 10 passed)
  notes: [prebuilt] same tag webrtc-89d790b, no fetch change; [keys] raw libwebrtc log still has 6 key lines per call, 0 pass our filter; [upstream] trusted rate controller and keyframe-gated deltas in the passthrough, PeerConnection::set_bitrate
Task 2: started
Task 2: built (commit "feat(video): add the video crate and the core's video API", gate ok: 1732 tests; stack call tests pass)
Task 2: complete (API shape approved by the human 2026-10-13; Phase 9 goes on with the Phase 6 call UI checkpoint open, human 2026-10-13)
  notes: [api] CameraPreview(VideoFeed) added beside the plan's events; NotSupported("video") for every video command until its task; [intent] JoinCall intent now written to membership and ring, read from incoming rings; [stale] engine/calls.rs is never compiled
