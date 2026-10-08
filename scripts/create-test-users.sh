#!/usr/bin/env bash
# Registers the test users from .env.test on the local homeserver, then empties
# their rooms and old test devices (see prune-test-rooms.sh and
# prune-test-devices.sh, need curl and jq). Idempotent. Run it
# between test runs, not during one.
# Never prints passwords or tokens.
set -euo pipefail
script_dir=$(cd "$(dirname "$0")" && pwd)
cd "$script_dir/.."
set -a
# shellcheck disable=SC1091
source .env.test
set +a
# shellcheck source=scripts/require-local-homeserver.sh
source "$script_dir/require-local-homeserver.sh"
require_local_homeserver

for _ in $(seq 60); do
    curl -sf "$HOMESERVER_URL/_matrix/client/versions" >/dev/null && break
    sleep 1
done
curl -sf "$HOMESERVER_URL/_matrix/client/versions" >/dev/null \
    || { echo "homeserver not reachable" >&2; exit 1; }

register() {
    local user=$1 password=$2 body code
    body=$(mktemp)
    trap 'rm -f "$body"' RETURN
    post() {
        curl -s -o "$body" -w '%{http_code}' -X POST \
            "$HOMESERVER_URL/_matrix/client/v3/register" \
            -H 'Content-Type: application/json' -d "$1"
    }
    local creds
    creds=$(printf '{"username":"%s","password":"%s","inhibit_login":true' "$user" "$password")
    code=$(post "$creds,\"auth\":{\"type\":\"m.login.dummy\"}}")
    if [ "$code" = 401 ]; then
        # Server wants a UIA session; retry with it.
        local session
        session=$(sed -n 's/.*"session" *: *"\([^"]*\)".*/\1/p' "$body" | head -n1)
        code=$(post "$creds,\"auth\":{\"type\":\"m.login.dummy\",\"session\":\"$session\"}}")
    fi
    if [ "$code" = 200 ]; then
        echo "registered $user"
    elif grep -q M_USER_IN_USE "$body"; then
        echo "$user already exists"
    else
        echo "failed to register $user (HTTP $code)" >&2
        return 1
    fi
}

register "$TEST_USER_1" "$TEST_PASSWORD_1"
register "$TEST_USER_2" "$TEST_PASSWORD_2"
register "$TEST_USER_3" "$TEST_PASSWORD_3"

# Rooms and devices left over from earlier runs slow every later test run
# down.
"$script_dir/prune-test-rooms.sh"
"$script_dir/prune-test-devices.sh"
