#!/usr/bin/env bash
# Signs out the devices earlier test runs left on alice's and carol's accounts
# on the local homeserver (each test login makes one): devices named as the
# tests name them, not seen for an hour. bob is left alone (reserved), and so
# is any device in use. Run it between test runs, never during one. Never
# prints passwords or tokens.
#
# Why: a call key goes to a device over an Olm session, and setting one up
# starts with the other user's whole device list (`/keys/query`). With
# thousands of leftover devices (2098 for alice after a few weeks) carol's key
# reached alice about 5 s after carol started talking, and
# `call_media::encrypted_room_calls_are_end_to_end_encrypted` heard silence.
set -euo pipefail
script_dir=$(cd "$(dirname "$0")" && pwd)
cd "$script_dir/.."
# The stack settings and test accounts: environment, .env.local, .env.test.
# shellcheck source=scripts/stack-env.sh
source "$script_dir/stack-env.sh"
load_stack_env
# shellcheck source=scripts/require-local-homeserver.sh
source "$script_dir/require-local-homeserver.sh"
require_local_homeserver

api="$HOMESERVER_URL/_matrix/client/v3"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# Devices to sign out at most per request.
chunk=200

prune() {
    local user=$1 password=$2 token session cutoff count failed=0
    # The password goes to jq through its environment, not its argv.
    token=$(PRUNE_USER=$user PRUNE_PASSWORD=$password jq -n \
        '{type:"m.login.password",identifier:{type:"m.id.user",user:env.PRUNE_USER},password:env.PRUNE_PASSWORD,initial_device_display_name:"tessera-prune"}' \
        | curl -sf -X POST "$api/login" -H 'Content-Type: application/json' -d @- \
        | jq -r .access_token)
    printf 'Authorization: Bearer %s\n' "$token" >"$work/auth"
    cutoff=$((($(date +%s) - 3600) * 1000))
    curl -sf "$api/devices" -H @"$work/auth" \
        | jq -r --argjson cutoff "$cutoff" '.devices[]
            | select(.display_name == "Tessera" or .display_name == "tessera-test-setup"
                     or .display_name == "tessera-prune")
            | select((.last_seen_ts // 0) < $cutoff)
            | .device_id' >"$work/devices"
    count=$(wc -l <"$work/devices")
    split -l "$chunk" "$work/devices" "$work/part."
    for part in "$work"/part.*; do
        [ -e "$part" ] || continue
        # User-interactive auth: the first request returns a session, the
        # second proves the password with it.
        session=$(jq -R . "$part" | jq -sc '{devices: .}' \
            | curl -s -X POST "$api/delete_devices" -H @"$work/auth" \
                -H 'Content-Type: application/json' -d @- \
            | jq -r '.session // empty')
        if ! jq -R . "$part" \
            | PRUNE_USER=$user PRUNE_PASSWORD=$password PRUNE_SESSION=$session jq -sc \
                '{devices: ., auth: {type:"m.login.password",identifier:{type:"m.id.user",user:env.PRUNE_USER},password:env.PRUNE_PASSWORD,session:env.PRUNE_SESSION}}' \
            | curl -sf -X POST "$api/delete_devices" -H @"$work/auth" \
                -H 'Content-Type: application/json' -d @- >/dev/null; then
            failed=$((failed + $(wc -l <"$part")))
        fi
        rm -f "$part"
    done
    # Signs out this session's own device too.
    curl -sf -X POST "$api/logout" -H @"$work/auth" -d '{}' >/dev/null
    echo "$user signed out $((count - failed)) old test devices"
    if [ "$failed" -ne 0 ]; then
        echo "$user: $failed devices could not be signed out" >&2
        failed_total=$((failed_total + failed))
    fi
}

# Not called with "||", so that "set -e" stays active inside prune.
failed_total=0
prune "$TEST_USER_1" "$TEST_PASSWORD_1"
prune "$TEST_USER_3" "$TEST_PASSWORD_3"
[ "$failed_total" -eq 0 ]
