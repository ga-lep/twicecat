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
exec /start.py run
