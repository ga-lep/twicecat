#!/bin/sh
# Dev-only Synapse bootstrap: open registration, localhost only.
set -eu
if [ ! -f /data/homeserver.yaml ]; then
    /start.py generate
    # Self-signed certificate for the federation listener on 8448. Only
    # lk-jwt-service uses it (it validates OpenID tokens over federation,
    # always via https://localhost:8448), and 8448 is not published.
    openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj /CN=localhost \
        -keyout /data/localhost.tls.key -out /data/localhost.tls.crt 2>/dev/null
    # start.py drops to UID:GID (image default 991:991) before running Synapse.
    chown "${UID:-991}:${GID:-991}" /data/localhost.tls.key /data/localhost.tls.crt
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
# Calls restart their delayed leave event every few seconds. A delayed event
# that hits rc_message is dropped by Synapse, not retried, so rc_message must
# stay relaxed too (it is, with the other limits below).
rc_delayed_event_mgmt:
  per_second: 1000
  burst_count: 1000
# Replaces the generated listeners (YAML: the last key wins). 8008 is the
# generated one unchanged; 8448 adds TLS federation for lk-jwt-service.
listeners:
  - port: 8008
    tls: false
    type: http
    x_forwarded: true
    resources:
      - names: [client, federation]
        compress: false
  - port: 8448
    tls: true
    type: http
    resources:
      - names: [federation]
        compress: false
tls_certificate_path: /data/localhost.tls.crt
tls_private_key_path: /data/localhost.tls.key
# MatrixRTC: MSC4143 transport discovery (/rtc/transports) pointing at the
# local lk-jwt-service, and MSC4140 delayed events for crash-safe call leave.
experimental_features:
  msc4143_enabled: true
matrix_rtc:
  transports:
    - type: livekit
      livekit_service_url: http://127.0.0.1:8080
max_event_delay_duration: 24h
YAML
fi
# The call service as clients reach it (TWICECAT_LK_JWT_URL, from the stack
# settings): set on every start, so a changed port applies to an existing
# volume too.
sed -i "s|^      livekit_service_url:.*|      livekit_service_url: ${TWICECAT_LK_JWT_URL:?}|" /data/homeserver.yaml
# The tests share three accounts and create rooms, invites and messages from
# the same user in bursts. Synapse's defaults (e.g. 10 room creations, then
# one per minute) answer with 429 and the SDK sleeps out `retry_after`, which
# stalled tests for minutes. Added separately so existing volumes get it too.
if ! grep -q '^# twicecat-dev-rate-limits' /data/homeserver.yaml; then
    cat >> /data/homeserver.yaml <<'YAML'
# twicecat-dev-rate-limits
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
