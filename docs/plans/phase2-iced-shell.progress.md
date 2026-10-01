# phase2-iced-shell progress

Plan approved 2026-10-02 under the human's standing pre-authorization (recommended action adopted while the human was asleep); Q1-Q22 to revisit at checkpoint.
Task 1: started
Task 1: complete (2a4edba..1440bcf, gate ok, security PASS, review PASS)
  notes: security Low — avatar decoding (Tasks 5/11) must use image::ImageReader Limits (max width/height/alloc); open::that (Task 9) must allow only https (+http loopback). review — shellcheck not run (not installed; human item), consider adding to lint CI; flate2 lock line names miniz_oxide 0.9.1 (two versions coexist, disclosed).
Task 2: started
Task 2: complete (cba8b4a..51cd5e7, gate ok, security PASS, review PASS)
  notes: security Low — FetchAvatars takes unbounded id list (cap/dedupe in Task 5); Info — Task 5 must validate mxc via OwnedMxcUri and fetch only via SDK media API; cap AvatarImage bytes and decoded size; CLI must sanitize() names/previews if it ever prints them; MessagePreview Debug shows sender_name (allowed). review — avatar Debug test asserts !contains("ab") (fragile but fine).
Task 3: started
Task 3: complete (add455c..373d3da, gate ok, security PASS, review PASS)
  notes: security Low — preview sender_name (and pre-existing room names) not sanitized (bidi/newline spoofing); preview_body misses U+2028/2029, U+200B-200D, U+FEFF, U+00AD. Info/review — map_batch does sequential local store work per room inside the forwarder (delays batch delivery, not the UI thread); if checkpoint shows lag, emit first then fill previews via buffer_unordered. review suggestions — first_sight allocs on every Set; leading blank line gives empty preview, tabs dropped; loaded test waits from LoggedIn not Running; Registered only cleared on task end.
Task 4: started
Task 4: complete (c92618f..4a4142c, gate ok, security PASS, review PASS)
  notes: gate round 1 failed (integration timeout dm_and_space_flags, 2nd distinct room_list timeout); debugger 4a4142c: root causes Synapse 429 rate limits (dev start.sh now 1000/s, local test server only) and create_dm helper clobbering m.direct (now merges under a process-wide lock); room_list file 189 s -> ~12-16 s, 3 consecutive passes. Local test Synapse volume was reset by controller (docker compose down -v; disposable test data). Open issue — a DM seen before its m.direct (e.g. self-created DM) stays is_dm=false/wrong tab until the list is rebuilt; fix later by re-emitting Set on m.direct change, or human-checkpoint note. security Low — search text length uncapped (optional 256-char cap). review warnings — rate-limit change is a deviation for human to see; create_dm lock is per test binary only.
Task 5: started
Task 5: complete (c0aada0..b3e34d0, gate ok, security PASS, review PASS)
  notes: security round 1 FAIL [High: File fallback + SDK Duration::MAX timeout => unbounded downloads/stalls]; fixed in b3e34d0 (thumbnail-only, 20 s deadline, *.localhost refused, failed fetches retryable, seen capped 4096) — deviation from Q11 recorded in PROGRESS.md + human checkpoint. Re-audit PASS with: Medium — failed fetches retried with no backoff (retry storm if the app re-requests failures; app (Task 11) must not loop on failed avatars, or add core backoff); Low — clearing seen at 4096 forgets in-flight ids, queued tasks uncapped; Low — response memory bounded only by the 20 s deadline (needs untrusted homeserver); Info — tests/avatars.rs disk scan doesn't prove encryption (assert no file contains bytes). review warnings — PROGRESS.md Task 5 notes lines ~316/318 still describe the removed File fallback, test count stale (fix in Task 11 wrap-up).
Task 6: started
Task 6: complete (1a9f51a..b5df2e5, gate ok, security PASS, review PASS)
  notes: security Low — --ephemeral dir removed only on normal exit (use RAII guard / reword --help); ephemeral root created recursively without AlreadyExists check; app tests/bridge.rs honours env HOMESERVER_URL and would send test creds over plain http to any host (assert loopback, AGENTS rule 6); Info — test temp dir left on failure, ensure_one_room grows local state, worst-case close ~15 s (human checkpoint). review — accepted deviations: App::booter(Startup), Bridge::start -> io::Result, Pending::deliver test, raw HTTP/1.0 room seeding; warnings: Bridge drop blocks UI up to 5 s at exit (spec), bridge test `next` conflates end/timeout; suggestions: share try_send fallback helper, doc TITLE. actionlint not installed (CI edit unlinted).
Task 7: started
Task 7: complete (bd84eff..cf91539, gate ok, security PASS, review PASS)
  notes: timing medians (debug, local) Reset 2000 1.84 ms, 4x Append 500 2.06 ms, mixed 0.43 ms, Clear+Reset 0.63 ms; timed region covers update only. Follow-up: extend timing test with avatar/paging bookkeeping in Tasks 10/11. security Low — app timeline Vec uncapped (cap in core if needed); quick A->B->A reselect can briefly apply stale same-room diffs before Reset (generation counter optional). review — Message::SelectRoom added (accepted); reselecting the same room clears timeline + resends (guard suggested); no core deselect when the selected room is removed (stale diffs ignored).
Task 8: started
Task 8: complete (04a39db..fcf6c89, gate ok, security PASS, review PASS)
  notes: review — Theme::custom allocates on every redraw (cache if it shows in profiling); semantic() keys on is_dark only. security — none (WCAG 2.1 0.03928 cutoff, harmless).
Task 9: started
