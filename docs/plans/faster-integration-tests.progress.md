Task 1: done (ci-speed.1); stack-dependent checks are HUMAN items in PROGRESS.md
Task 1: complete (7e86efd..a97e0ad, gate ok, security PASS, review PASS, evidence PROVEN (offline parts only))
  evidence: docs/plans/faster-integration-tests.evidence/task-1.md
  notes: HUMAN: timed run, --per-test, --first-sync, CI step durations, shellcheck not run (not installed). Security Low: first_sync_counts leaves token file and a logged-in device if a curl fails mid-way (trap/logout with || true); test logs under target/ created without umask 077. Review: no message on login failure under set -e; header should say it does not prune rooms first; print log path on FAILED. Task 11 or a later fixup may fold these in.
