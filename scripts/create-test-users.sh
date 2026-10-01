#!/usr/bin/env bash
# Registers the test users from .env.test on the local homeserver. Idempotent.
# Never prints passwords or tokens.
set -euo pipefail
cd "$(dirname "$0")/.."
set -a
# shellcheck disable=SC1091
source .env.test
set +a

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
