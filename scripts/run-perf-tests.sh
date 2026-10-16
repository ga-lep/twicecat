#!/usr/bin/env bash
# Runs the performance indicators: every test named perf_ (docs/testing.md, tier
# 4). Each is run alone (--test-threads=1), because a rate measured while other
# tests run measures the contention, not the code. A value outside its
# expectation is reported, never an error: this script fails only when a perf_
# test panics (a behaviour broke) or when none ran.
#
#   scripts/run-perf-tests.sh             all of them; the #[ignore] ones need the
#                                         local homeserver (scripts/stack.sh up -d
#                                         --wait && scripts/create-test-users.sh)
#   scripts/run-perf-tests.sh --no-stack  only those that need no homeserver
#   scripts/run-perf-tests.sh --check     fails unless every test file with a perf_
#                                         test is listed below (reads sources only)
#
# The values go to target/perf/<UTC timestamp>.jsonl ($TWICECAT_PERF_FILE
# overrides it); scripts/perf-report.sh turns the file into a table. Only the
# binaries listed here are built, as in integration-shards.sh.
set -euo pipefail
cd "$(dirname "$0")/.."

# <package>:<binary>, the binary being the test file name without .rs, or `lib`
# for the package's unit tests.
PERF_TARGETS="twicecat-app:lib twicecat-core:call_video twicecat-core:cli twicecat-video:lib"

check() {
    local failed=0 f pkg name entry
    local found
    found=$(
        for f in $(find crates -path 'crates/*/src/*' -name '*.rs' | sort); do
            grep -q 'fn perf_' "$f" || continue
            echo "twicecat-$(echo "$f" | cut -d/ -f2):lib"
        done
        for f in crates/*/tests/*.rs; do
            grep -q 'fn perf_' "$f" || continue
            pkg=twicecat-$(echo "$f" | cut -d/ -f2)
            echo "$pkg:$(basename "$f" .rs)"
        done
    )
    found=$(printf '%s\n' "$found" | sort -u)
    for entry in $found; do
        printf '%s\n' $PERF_TARGETS | grep -qxF "$entry" || { echo "perf_ test in no PERF_TARGETS entry: $entry" >&2; failed=1; }
    done
    for entry in $PERF_TARGETS; do
        printf '%s\n' "$found" | grep -qxF "$entry" || { echo "PERF_TARGETS lists $entry, which has no perf_ test" >&2; failed=1; }
    done
    [ "$failed" -eq 0 ] && echo "perf targets ok: $(printf '%s\n' $PERF_TARGETS | wc -l) binaries"
    return "$failed"
}

stack=1
case ${1:-} in
    --check) check; exit ;;
    --no-stack) stack=0 ;;
    "") ;;
    *) echo "usage: $0 [--no-stack | --check]" >&2; exit 2 ;;
esac

# As run-integration-tests.sh: a call test holds thousands of descriptors.
ulimit -n "$(ulimit -Hn)" 2>/dev/null || true
if [ "$stack" -eq 1 ]; then
    scripts/prune-test-rooms.sh
fi

stamp=$(date -u +%Y%m%dT%H%M%SZ)
export TWICECAT_PERF_FILE=${TWICECAT_PERF_FILE:-$PWD/target/perf/$stamp.jsonl}
mkdir -p "$(dirname "$TWICECAT_PERF_FILE")"
: > "$TWICECAT_PERF_FILE"

declare -A selectors=()
for entry in $PERF_TARGETS; do
    pkg=${entry%%:*}
    name=${entry#*:}
    if [ "$name" = lib ]; then
        selectors[$pkg]+=" --lib"
    else
        selectors[$pkg]+=" --test $name"
    fi
done

args=(perf_ --skip hardware_ --skip keyring --test-threads=1 --nocapture)
[ "$stack" -eq 1 ] && args+=(--include-ignored)

log=$(mktemp)
trap 'rm -f "$log"' EXIT
for pkg in "${!selectors[@]}"; do
    # shellcheck disable=SC2086
    cargo test -p "$pkg" ${selectors[$pkg]} --locked -- "${args[@]}" 2>&1 | tee -a "$log"
done
ran=$(awk '/^test result: ok\./ { n += $4 } END { print n + 0 }' "$log")
if [ "$ran" -eq 0 ]; then
    echo "error: no perf_ test ran" >&2
    exit 1
fi
echo "perf: $ran tests ran; $(wc -l < "$TWICECAT_PERF_FILE") values in $TWICECAT_PERF_FILE"
