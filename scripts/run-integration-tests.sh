#!/usr/bin/env bash
# Runs the ignored homeserver tests the way the agent gate and CI do, after
# emptying the test users' rooms: leftover rooms from earlier runs make a
# core's first sync slow (see prune-test-rooms.sh) and push the suite towards
# its time limit. Rooms only: devices and identities carry over between runs,
# so the repeat-run recipe holds (create-test-users.sh resets them).
# Needs the local homeserver (docker compose up -d --wait), curl and jq.
# Extra arguments go to the test binaries, after the built-in ones.
set -euo pipefail
script_dir=$(cd "$(dirname "$0")" && pwd)
cd "$script_dir/.."

"$script_dir/prune-test-rooms.sh"
exec cargo test -p twicecat-core -p twicecat-app --locked -- \
    --ignored --skip keyring --skip hardware_ "$@"
