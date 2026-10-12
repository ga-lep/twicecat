#!/usr/bin/env bash
# Runs the ignored homeserver tests the way the agent gate and CI do, after
# emptying the test users' rooms: leftover rooms from earlier runs make a
# core's first sync slow (see prune-test-rooms.sh) and push the suite towards
# its time limit. Rooms only: devices and identities carry over between runs,
# so the repeat-run recipe holds (create-test-users.sh resets them).
# Needs the local homeserver (docker compose up -d --wait), curl and jq.
# Extra arguments go to the test binaries, after the built-in ones.
# With `--shard N` (first argument; N is 1-4) only the test binaries of that CI
# shard run, listed in integration-shards.sh; without it, all of them run.
set -euo pipefail
script_dir=$(cd "$(dirname "$0")" && pwd)
cd "$script_dir/.."
# A call test alone holds over 2500 descriptors; a shell limit of 1024 makes
# its HTTP clients fail ("No CA certificates were loaded"). Raise the soft
# limit to the hard one; if that is refused, the limit stays as it is.
ulimit -n "$(ulimit -Hn)" 2>/dev/null || true

shard=
if [ "${1:-}" = --shard ]; then
    shard=${2:?usage: --shard N}
    shift 2
fi

"$script_dir/prune-test-rooms.sh"
test_args=(--ignored --skip keyring --skip hardware_ "$@")
if [ -z "$shard" ]; then
    exec cargo test -p twicecat-core -p twicecat-app --locked -- "${test_args[@]}"
fi

# One cargo call per package: --test names a target of every selected package.
declare -A selectors=()
for entry in $("$script_dir/integration-shards.sh" --list "$shard"); do
    pkg=${entry%%:*}
    name=${entry#*:}
    if [ "$name" = lib ]; then
        selectors[$pkg]+=" --lib"
    else
        selectors[$pkg]+=" --test $name"
    fi
done
# Run every binary of the shard even when one fails, so a run reports all the
# failures of the shard, then fail if any package's run failed.
status=0
for pkg in "${!selectors[@]}"; do
    # shellcheck disable=SC2086
    cargo test -p "$pkg" ${selectors[$pkg]} --locked --no-fail-fast -- "${test_args[@]}" || status=1
done
exit "$status"
