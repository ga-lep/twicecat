Plan: drafted 2026-10-12 for ga-lep/twicecat#14; questions Q1-Q8 answered and plan approved by the human 2026-10-12
Task 1: started
Task 1: complete (commit "feat(app): keep the call, rings and audio settings from the core's events", gate ok: fmt, clippy (also --all-features), 1593 tests passed, dependency/API/commit checks)
  notes: [test] the rooms_ui test that expected no command after login drains GetAudioSettings first; [layout] tests inline in calls.rs (no_secret_logging scans tests.rs files as app code); [display] newlines in a failure reason show as ⏎ (one_line_capped)
Task 2: started
Task 2: complete (commit "feat(app): show calls in the room list and a call button in the header", gate ok: 1603 tests passed)
  notes: [style] primary/danger buttons; success avoided (not contrast-checked); [pending] one join or leave in flight at a time, cleared by any CallState or call error
