# Evidence — Task 1: core cleaning give-up tests
Commit: "feat(core): cover the switch from a failing GPU bank to the CPU" (subject; SHAs were rewritten when the history was tidied)
Environment: local, CC=clang-22 CXX=clang++-22; models fetched by scripts/fetch-models.sh into ~/.cache/twicecat/models/deepfilternet3 (outside the repo).

## New cleaning tests drive a VoiceEngine with a fake bank (give-up after GPU error and too-slow, reload after gpu off/on, row freed)
Status: PROVEN
```console
$ cargo test -p twicecat-core --lib call::cleaning
test call::cleaning::tests::the_cpu_filters_take_over_after_a_give_up ... ignored, needs scripts/fetch-models.sh
test call::cleaning::tests::a_voice_that_leaves_frees_its_row ... ok
test call::cleaning::tests::a_bank_too_slow_says_so ... ok
test call::cleaning::tests::a_bank_that_fails_hands_its_voices_to_the_cpu ... ok
test call::cleaning::tests::the_gpu_setting_turned_off_and_on_loads_a_new_bank_after_a_give_up ... ok
test result: ok. 12 passed; 0 failed; 1 ignored; 0 measured; 466 filtered out
```
(policy tests omitted from the excerpt; all 12 passed.) The four new tests pass.

## Ignored CPU-takeover test (needs models)
Status: PROVEN
```console
$ scripts/fetch-models.sh
/home/galep/.cache/twicecat/models/deepfilternet3
$ TWICECAT_MODEL_DIR=... cargo test -p twicecat-core --lib -- --ignored call::cleaning
test call::cleaning::tests::the_cpu_filters_take_over_after_a_give_up ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 478 filtered out
```

## Public core API unchanged
Status: PROVEN
```console
$ scripts/check-core-api.sh; echo exit=$?
exit=0
```

## CleaningAnnouncement rustdoc has the privacy note
Status: PROVEN
```console
$ git diff <main>..<"feat(core): cover the switch from a failing GPU bank to the CPU" commit> -- crates/core/src/api/audio.rs
+/// Privacy: the attribute (which suppressor runs on your microphone, or
+/// that it is cleaned outside Twicecat) is seen by everyone in the call and
+/// by the operator of the SFU, like any other participant attribute. Turn
+/// [`enabled`](Self::enabled) off to stop sending it.
```

## PROGRESS.md step 16 ticked
Status: PROVEN
```console
$ git diff <main>..<"feat(core): cover the switch from a failing GPU bank to the CPU" commit> -- PROGRESS.md | grep '16\. Cleaning'
-- [ ] 16. Cleaning other people's voices, opt-in (added 2026-10-03; ...
+- [x] 16. Cleaning other people's voices, opt-in (added 2026-10-03; ...
```

## Verdict
EVIDENCE: PROVEN
