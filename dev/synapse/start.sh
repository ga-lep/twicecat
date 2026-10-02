#!/bin/sh
# Dev-only Synapse bootstrap: open registration, localhost only.
set -eu
if [ ! -f /data/homeserver.yaml ]; then
    /start.py generate
    echo >> /data/homeserver.yaml  # generated file has no trailing newline
    cat >> /data/homeserver.yaml <<'YAML'
enable_registration: true
enable_registration_without_verification: true
rc_registration:
  per_second: 1000
  burst_count: 1000
# Integration tests log in from one address many times in a few seconds.
rc_login:
  address:
    per_second: 1000
    burst_count: 1000
  account:
    per_second: 1000
    burst_count: 1000
  failed_attempts:
    per_second: 1000
    burst_count: 1000
YAML
fi
# The tests share three accounts and create rooms, invites and messages from
# the same user in bursts. Synapse's defaults (e.g. 10 room creations, then
# one per minute) answer with 429 and the SDK sleeps out `retry_after`, which
# stalled tests for minutes. Added separately so existing volumes get it too.
if ! grep -q '^# tessera-dev-rate-limits' /data/homeserver.yaml; then
    cat >> /data/homeserver.yaml <<'YAML'
# tessera-dev-rate-limits
rc_message:
  per_second: 1000
  burst_count: 1000
rc_room_creation:
  per_second: 1000
  burst_count: 1000
rc_joins:
  local:
    per_second: 1000
    burst_count: 1000
  remote:
    per_second: 1000
    burst_count: 1000
rc_invites:
  per_room:
    per_second: 1000
    burst_count: 1000
  per_user:
    per_second: 1000
    burst_count: 1000
  per_issuer:
    per_second: 1000
    burst_count: 1000
rc_3pid_validation:
  per_second: 1000
  burst_count: 1000
YAML
fi
exec /start.py run
