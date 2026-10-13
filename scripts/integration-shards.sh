#!/usr/bin/env bash
# The CI shards of the ignored homeserver tests: which test binaries each of the
# 4 `integration` jobs runs. Entries are <package>:<binary>, where <binary> is
# the test file name without .rs, or `lib` for the package's unit tests.
#
# Usage:
#   scripts/integration-shards.sh --list N   prints the entries of shard N (1-4)
#   scripts/integration-shards.sh --check    fails unless every test file with
#                                            ignored tests, and every package
#                                            whose src has ignored tests, is in
#                                            exactly one shard (or in
#                                            NOT_SHARDED below); needs no stack
#
# The shards are balanced by the seconds of the baseline in
# docs/plans/faster-integration-tests.progress.md (about 325 s each). A new test
# file with `#[ignore` fails --check until it is placed here. --check reads the
# sources only: it does not know the `--skip keyring --skip hardware_` filter,
# so a binary whose ignored tests are all skipped goes in NOT_SHARDED.
set -euo pipefail

SHARD_1="twicecat-core:media twicecat-core:notifications twicecat-core:timeline_live twicecat-core:timeline_items twicecat-core:timeline_actions twicecat-core:oauth twicecat-core:rtc_stack"
SHARD_2="twicecat-core:e2ee_verification twicecat-app:bridge twicecat-core:e2ee_devices twicecat-core:call_media twicecat-core:room_list twicecat-core:login"
SHARD_3="twicecat-core:avatars twicecat-core:e2ee_identity twicecat-core:e2ee_rooms twicecat-core:lib twicecat-core:timeline twicecat-core:call_ringing twicecat-core:call_log_hygiene twicecat-core:call_video"
SHARD_4="twicecat-core:rooms twicecat-core:e2ee_recovery twicecat-core:call_signalling twicecat-core:log_hygiene twicecat-core:threads twicecat-core:cli twicecat-app:calls"
# Their ignored tests are all keyring/hardware_ ones (skipped) or none runs.
NOT_SHARDED="twicecat-core:livekit_link twicecat-core:call_settings twicecat-core:gpu_enhancement twicecat-core:login_policy"

shard_entries() {
    case $1 in
        1) echo "$SHARD_1" ;;
        2) echo "$SHARD_2" ;;
        3) echo "$SHARD_3" ;;
        4) echo "$SHARD_4" ;;
        *) echo "unknown shard: $1 (expected 1-4)" >&2; return 2 ;;
    esac
}

check() {
    cd "$(dirname "$0")/.."
    local all="$SHARD_1 $SHARD_2 $SHARD_3 $SHARD_4" failed=0 entry dup f pkg name
    for entry in $all; do
        dup=$(printf '%s\n' $all $NOT_SHARDED | grep -cxF "$entry")
        if [ "$dup" -gt 1 ]; then echo "listed twice: $entry" >&2; failed=1; fi
    done
    for f in crates/core/tests/*.rs crates/app/tests/*.rs; do
        grep -q '#\[ignore' "$f" || continue
        pkg=twicecat-$(basename "$(dirname "$(dirname "$f")")")
        name=$(basename "$f" .rs)
        if ! printf '%s\n' $all $NOT_SHARDED | grep -qxF "$pkg:$name"; then
            echo "in no shard: $pkg:$name ($f)" >&2
            failed=1
        fi
    done
    for entry in $all $NOT_SHARDED; do
        pkg=${entry%%:*}
        name=${entry#*:}
        if [ "$name" = lib ]; then
            grep -rq '#\[ignore' "crates/${pkg#twicecat-}/src" || { echo "no ignored tests in $entry" >&2; failed=1; }
        else
            [ -f "crates/${pkg#twicecat-}/tests/$name.rs" ] || { echo "no such test file: $entry" >&2; failed=1; }
        fi
    done
    grep -rq '#\[ignore' crates/core/src && ! printf '%s\n' $all | grep -qxF twicecat-core:lib \
        && { echo "in no shard: twicecat-core:lib" >&2; failed=1; }
    [ "$failed" -eq 0 ] && echo "shards ok: $(printf '%s\n' $all | wc -l) binaries in 4 shards"
    return "$failed"
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    case ${1:-} in
        --list) shard_entries "${2:?usage: --list N}" | tr ' ' '\n' ;;
        --check) check ;;
        *) sed -n '2,/^set -/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 2 ;;
    esac
fi
