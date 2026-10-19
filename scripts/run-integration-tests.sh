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
# `--no-run` (after --shard) builds them without running anything and needs no
# homeserver: CI builds a shard's binaries in one step and runs them in the next,
# with the same cargo calls, so the run step does not compile again.
set -euo pipefail
script_dir=$(cd "$(dirname "$0")" && pwd)
cd "$script_dir/.."
# A call test alone holds over 2500 descriptors; a shell limit of 1024 makes
# its HTTP clients fail ("No CA certificates were loaded"). Raise the soft
# limit to the hard one; if that is refused, the limit stays as it is.
ulimit -n "$(ulimit -Hn)" 2>/dev/null || true

shard=
no_run=
if [ "${1:-}" = --shard ]; then
    shard=${2:?usage: --shard N}
    shift 2
fi
if [ "${1:-}" = --no-run ]; then
    no_run=--no-run
    shift
fi

# Rooms are only emptied for a run.
[ -n "$no_run" ] || "$script_dir/prune-test-rooms.sh"
# perf_ tests are indicators and run in the perf workflow (docs/testing.md).
test_args=(--ignored --skip keyring --skip hardware_ --skip perf_ "$@")
if [ -z "$shard" ]; then
    # shellcheck disable=SC2086
    exec cargo test -p twicecat-core -p twicecat-app --locked $no_run -- "${test_args[@]}"
fi

# One cargo call for the whole shard, for both packages: --test names a target of
# whichever selected package has it (integration-shards.sh --check makes sure a
# name is in one package only), and one call builds the shared dependencies once
# (a call per package built them again under another feature set). `lib` selects
# the unit tests of both packages; the app has no ignored ones, so it adds no tests.
selectors=()
for entry in $("$script_dir/integration-shards.sh" --list "$shard"); do
    name=${entry#*:}
    if [ "$name" = lib ]; then
        selectors+=(--lib)
    else
        selectors+=(--test "$name")
    fi
done
# --no-fail-fast: every binary of the shard runs even when one fails, so a run
# reports all the failures of the shard.
# shellcheck disable=SC2086
exec cargo test -p twicecat-core -p twicecat-app "${selectors[@]}" --locked --no-fail-fast $no_run -- "${test_args[@]}"
