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
