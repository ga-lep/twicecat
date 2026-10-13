Task 1: complete (afd86cc..6695175, gate ok, security PASS, review PASS, evidence N/A)
  evidence: docs/plans/ui-ux-rework.evidence/task-1.md
  notes: icon scan misses inline \u{...} escapes (left for screen tasks); Severity::Danger uses Icon::Close (confusable with dismiss); modal hardcodes padding 20; tooltip reuses card radius; build needs CC=clang-22 CXX=clang++-22
Task 2: complete (6a6c5d6..e7969ed, gate ok, security PASS, review PASS, evidence PROVEN)
  evidence: docs/plans/ui-ux-rework.evidence/task-2.md
  notes: first review FAILED (font check not in CI) -> fixed and folded; font hashes live next to files (CODEOWNERS/upstream re-check job suggested); shellcheck unavailable (human); Font::DEFAULT grep test splits at first #[cfg(test)] so timeline_pane.rs prod code unscanned; release size +1,676,472 B unverified by evidence
Task 3: complete (65c72b0..6a58698, gate ok, security PASS, review PASS, evidence PROVEN)
  evidence: docs/plans/ui-ux-rework.evidence/task-3.md
  notes: UiSettings/ThemeChoice lost Deserialize (not purely additive; unknown theme now falls back instead of marking file bad); write_settings does not clamp text_scale_percent (read does); settings.json read follows symlinks and .bad rename overwrites (pre-existing); fallback test lacks unknown-theme case
Task 4: complete (ec68957..4332f03, gate ok, security PASS, review PASS, evidence PROVEN)
  evidence: docs/plans/ui-ux-rework.evidence/task-4.md
  notes: Zoom::In lacks saturating_add; Ctrl+Shift+= and numpad plus unbound; red step was compile failure for most tests; GUI session, settings.json key removal end to end and a resize-keeps-heights test not shown (human checkpoint / gap); one settings write per zoom step
Task 5: complete (095ed12..942366b, gate ok, security PASS, review PASS, evidence PROVEN)
  evidence: docs/plans/ui-ux-rework.evidence/task-5.md
  notes: TWICECAT_GALLERY_DIR unvalidated path (Low, opt-in test); unplanned dev-profile opt-level=3 list in root Cargo.toml (no version change); -rail PNGs identical to expanded until Task 7; no_secret_logging (needs homeserver) not run; layout glitches seen in gallery for later tasks: "Leave call" wraps/clipped in header, "Audio..." cut off in call bar, Voice&audio Close/Set key under scrollbar, low-contrast login button labels; gallery view: TWICECAT_GALLERY_DIR=target/ui-gallery cargo test -p twicecat-app --test ui_gallery -- --ignored
