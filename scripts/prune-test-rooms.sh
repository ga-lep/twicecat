#!/usr/bin/env bash
# Makes every test user leave and forget all rooms and decline all invites on
# the local homeserver, so that the rooms earlier test runs created do not pile
# up. Run it between test runs, never during one (it would pull rooms out from
# under running tests). Never prints passwords, tokens or room names.
#
# Why: a core that logs in as a test user processes every joined room on its
# first sync, and the SDK's event cache needs roughly 0.1 s per room in a
# debug build. After a few hundred leftover rooms that start-up work outlasts
# the 30 s wait of the preview test.
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

api="$HOMESERVER_URL/_matrix/client/v3"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Keeps the sync response small. Only invites are read from it: an initial
# sync may leave joined rooms with no events out, `joined_rooms` lists them all.
filter='{"presence":{"types":[]},"account_data":{"types":[]},"room":{"timeline":{"limit":0},"state":{"types":[]},"ephemeral":{"types":[]},"account_data":{"types":[]}}}'

prune() {
    local user=$1 password=$2 token count=0 failed=0 room
    # The password goes to jq through its environment, not its argv.
    token=$(PRUNE_USER=$user PRUNE_PASSWORD=$password jq -n \
        '{type:"m.login.password",identifier:{type:"m.id.user",user:env.PRUNE_USER},password:env.PRUNE_PASSWORD,initial_device_display_name:"tessera-prune"}' \
        | curl -sf -X POST "$api/login" -H 'Content-Type: application/json' -d @- \
        | jq -r .access_token)
    printf 'Authorization: Bearer %s\n' "$token" >"$work/auth"
    {
        curl -sf "$api/joined_rooms" -H @"$work/auth" | jq -r '.joined_rooms[] | @uri'
        curl -sf -G "$api/sync" -H @"$work/auth" --data-urlencode "filter=$filter" \
            | jq -r '(.rooms.invite // {}) | keys[] | @uri'
    } >"$work/rooms"
    while IFS= read -r room; do
        if curl -sf -X POST "$api/rooms/$room/leave" -H @"$work/auth" -d '{}' >/dev/null \
            && curl -sf -X POST "$api/rooms/$room/forget" -H @"$work/auth" -d '{}' >/dev/null; then
            count=$((count + 1))
        else
            failed=$((failed + 1))
        fi
    done <"$work/rooms"
    # Always log out, even when some rooms failed.
    curl -sf -X POST "$api/logout" -H @"$work/auth" -d '{}' >/dev/null
    echo "$user left $count rooms"
    if [ "$failed" -ne 0 ]; then
        echo "$user: $failed rooms could not be left or forgotten" >&2
        failed_total=$((failed_total + failed))
    fi
}

# Not called with "||", so that "set -e" stays active inside prune.
failed_total=0
prune "$TEST_USER_1" "$TEST_PASSWORD_1"
prune "$TEST_USER_2" "$TEST_PASSWORD_2"
prune "$TEST_USER_3" "$TEST_PASSWORD_3"
[ "$failed_total" -eq 0 ]
