Plan: drafted 2026-10-12 for ga-lep/twicecat#14; questions Q1-Q8 answered and plan approved by the human 2026-10-12
Task 1: started
Task 1: complete (commit "feat(app): keep the call, rings and audio settings from the core's events", gate ok: fmt, clippy (also --all-features), 1593 tests passed, dependency/API/commit checks)
  notes: [test] the rooms_ui test that expected no command after login drains GetAudioSettings first; [layout] tests inline in calls.rs (no_secret_logging scans tests.rs files as app code); [display] newlines in a failure reason show as ⏎ (one_line_capped)
Task 2: started
Task 2: complete (commit "feat(app): show calls in the room list and a call button in the header", gate ok: 1603 tests passed)
  notes: [style] primary/danger buttons; success avoided (not contrast-checked); [pending] one join or leave in flight at a time, cleared by any CallState or call error
Task 3: started
Task 3: complete (commit "feat(app): show the call bar and the participants panel", gate ok: 1630 tests passed)
  notes: [iced] slider takes no keyboard focus, so −/+ buttons and a 400 ms settle timer (wheel has no release); [layout] the participants panel is a fifth slot of the panes row (the room pane tree is unchanged); [members] the call room's member list is kept by the call model even when another room is open
Task 4: started
Task 4: complete (commit "feat(core): play a ringtone while a direct call rings", gate ok; local stack: call_ringing 6 passed; full ignored run 145 passed, 7 failed, all needing scripts/fetch-models.sh, which was not run)
  notes: [design] the watcher publishes "a loud ring" on a watch channel; a per-session Ringer opens a playback-only AudioIo while the engine's gate allows it; [api] Command::SilenceRinging and AudioSettings::ringtone (Q3b); [tests] timing-based ringer tests poll with 5 s deadlines, 20 runs clean; [env] DeepFilterNet models not fetched here
Task 5: started
Task 5: complete (commit "feat(app): ring with a card, attention and a notification", gate ok; app stack test with twicecat-cli: 4 runs passed, a mutation (Decline sending nothing) caught)
  notes: [fix, folded into Task 2's commit per rule 5] Join no longer requires the room in the filtered room list (Accept with a search active would have done nothing); [layout] the ring card is the outermost layer of the main screen; [stack test] in crates/app/tests/calls.rs, alice is the real twicecat-cli binary next to the test binary
Task 6: started
Task 6: complete (commit "feat(app): add the Voice & audio card", gate ok)
  notes: [a11y] −/+ beside every slider (iced 0.14 sliders take no keyboard focus); [Q7] input mode and push-to-talk pass through untouched; [Q5] Settings closes when it opens the card, as for Sessions
Review: an independent read-only review of main..HEAD found 10 verified defects, privacy clean; 9 fixed and folded into their commits (--fixup, rebase --autosquash: clean, final tree identical, each rewritten commit's lib tests run again), 1 open (low: pending/answered cleared broadly, NotLoggedIn not an answer); the expired-session leak on main became "fix(core): end the call and the mic test when the session expires"
Task 7: complete (commit "docs(plan): wrap up the Phase 6 call UI")
Fix: complete (commit "fix(core): refuse a mic test during a call before touching its audio", gate ok: 1690 tests passed; full ignored run 147 passed, 7 failed, all needing scripts/fetch-models.sh)
  notes: [bug, on main] StartMicTest stopped the call's audio session before refusing; now refused first, also while the session is in Mode::Call; unit and stack tests both catch the mutation
Checks: with the DeepFilterNet3 models fetched (at the human's request), the full ignored run passed 154, failed 0; profile-call-path.sh: every mode under the 500 us budget (DeepFilterNet3 CPU p99 76.9 %)
Checks: GPU parts fetched (ONNX Runtime cuda13, CUDA runtime, stateful export) on the RTX 5080: audio GPU suites 9 passed, core GPU suites 9 passed (CUDA); real-speech test run (DFN 14.6 dB vs RNNoise 8.7 dB); all-features workspace 1694 passed; all-features ignored 158 passed, 2 failed outside this branch (rooms::members_have_roles_and_dms_are_reused flaky, also failing on main; one keyword-notification timeout that passed 3 reruns)
