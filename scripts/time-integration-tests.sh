#!/usr/bin/env bash
# Times the ignored homeserver tests: the build once, then every test binary
# that has ignored tests, run the way run-integration-tests.sh runs them.
# Writes target/integration-timings/<UTC timestamp>.tsv with rows
#   binary<TAB>seconds<TAB>result
# and, with --per-test, rows
#   binary<TAB>test<TAB>seconds<TAB>result
# for each ignored test run alone (--exact). Test output goes to a .log file
# beside the TSV, never to the terminal. Never prints passwords, tokens, room
# names or message bodies.
#
# Usage: scripts/time-integration-tests.sh [--per-test] [--first-sync] [--dry-run]
#   --per-test    also run each ignored test alone
#   --first-sync  print, per test user, the number of joined rooms (the count
#                 only; room ids are not printed). The first sync of a core
#                 grows with that number (see prune-test-rooms.sh).
#   --dry-run     build and list what would run; run no test, write no TSV
# The test users' rooms are pruned first, as run-integration-tests.sh does,
# so leftover rooms from earlier runs do not slow every core's first sync;
# --first-sync counts them just before that.
# Needs the local homeserver (scripts/stack.sh up -d --wait; then
# scripts/create-test-users.sh), jq, and curl for --first-sync.
set -euo pipefail
script_dir=$(cd "$(dirname "$0")" && pwd)
cd "$script_dir/.."

# A call test alone holds over 2500 descriptors; a shell limit of 1024 makes
# its HTTP clients fail ("No CA certificates were loaded"). Raise the soft
# limit to the hard one; if that is refused, the limit stays as it is.
ulimit -n "$(ulimit -Hn)" 2>/dev/null || true

per_test=0
first_sync=0
dry_run=0
for arg in "$@"; do
    case $arg in
        --per-test) per_test=1 ;;
        --first-sync) first_sync=1 ;;
        --dry-run) dry_run=1 ;;
        -h | --help)
            sed -n '2,/^set -/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown argument: $arg" >&2
            exit 2
            ;;
    esac
done

# shellcheck source=scripts/stack-env.sh
source "$script_dir/stack-env.sh"
load_stack_env
# shellcheck source=scripts/require-local-homeserver.sh
source "$script_dir/require-local-homeserver.sh"
require_local_homeserver

now() { date +%s.%N; }
elapsed() { awk -v a="$1" -v b="$2" 'BEGIN { printf "%.1f", b - a }'; }

# Prints the joined-room count per test user. The password goes to jq through
# its environment, not its argv; the token stays in a header file.
first_sync_counts() {
    local api="$HOMESERVER_URL/_matrix/client/v3" work user password token n i
    work=$(mktemp -d)
    for i in 1 2 3; do
        user_var=TEST_USER_$i
        pass_var=TEST_PASSWORD_$i
        user=${!user_var}
        password=${!pass_var}
        token=$(TIME_USER=$user TIME_PASSWORD=$password jq -n \
            '{type:"m.login.password",identifier:{type:"m.id.user",user:env.TIME_USER},password:env.TIME_PASSWORD,initial_device_display_name:"twicecat-timing"}' \
            | curl -sf -X POST "$api/login" -H 'Content-Type: application/json' -d @- \
            | jq -r .access_token)
        printf 'Authorization: Bearer %s\n' "$token" >"$work/auth"
        n=$(curl -sf "$api/joined_rooms" -H @"$work/auth" | jq '.joined_rooms | length')
        curl -sf -X POST "$api/logout" -H @"$work/auth" -d '{}' >/dev/null
        echo "first-sync: test user $i has $n joined rooms"
    done
    rm -rf "$work"
}

if [ "$first_sync" -eq 1 ] && [ "$dry_run" -eq 0 ]; then
    first_sync_counts
fi

if [ "$dry_run" -eq 0 ]; then
    "$script_dir/prune-test-rooms.sh"
fi

out_dir=target/integration-timings
stamp=$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p "$out_dir"
tsv="$out_dir/$stamp.tsv"
log="$out_dir/$stamp.log"
json=$(mktemp)
trap 'rm -f "$json"' EXIT

t0=$(now)
cargo test -p twicecat-core -p twicecat-app --no-run --locked --message-format=json >"$json"
t1=$(now)
echo "build: $(elapsed "$t0" "$t1") s"

# One line per test binary: name<TAB>cargo selector<TAB>manifest<TAB>executable.
targets=$(jq -r '
    select(.reason == "compiler-artifact" and .profile.test == true and .executable != null)
    | (.target.kind[0]) as $k
    | [ .target.name,
        (if $k == "test" then "--test " + .target.name
         elif $k == "bin" then "--bin " + .target.name
         else "--lib" end),
        .manifest_path, .executable ] | @tsv' "$json" | sort -u)

# The tests the integration run skips (see run-integration-tests.sh).
skip_args=(--skip keyring --skip hardware_)
skipped() { [[ $1 == *keyring* || $1 == *hardware_* ]]; }

[ "$dry_run" -eq 1 ] || : >"$tsv"

while IFS=$'\t' read -r name selector manifest exe; do
    [ -n "$name" ] || continue
    pkg_dir=$(dirname "$manifest")
    # The ignored tests of this binary, minus the skipped ones.
    tests=()
    while IFS= read -r line; do
        t=${line%: test}
        [ "$t" != "$line" ] || continue
        skipped "$t" || tests+=("$t")
    done < <(cd "$pkg_dir" && CARGO_MANIFEST_DIR=$pkg_dir "$exe" --list --ignored --format terse 2>/dev/null)
    [ "${#tests[@]}" -gt 0 ] || continue

    if [ "$dry_run" -eq 1 ]; then
        echo "would run: $name (${#tests[@]} ignored tests)"
        continue
    fi

    s=$(now)
    # $selector is "--test name" and must word-split.
    # shellcheck disable=SC2086
    if cargo test --manifest-path "$manifest" $selector --locked -- \
        --ignored "${skip_args[@]}" >>"$log" 2>&1; then result=ok; else result=FAILED; fi
    e=$(now)
    printf '%s\t%s\t%s\n' "$name" "$(elapsed "$s" "$e")" "$result" | tee -a "$tsv"

    if [ "$per_test" -eq 1 ]; then
        for t in "${tests[@]}"; do
            s=$(now)
            # shellcheck disable=SC2086
            if cargo test --manifest-path "$manifest" $selector --locked -- \
                --ignored --exact "$t" >>"$log" 2>&1; then result=ok; else result=FAILED; fi
            e=$(now)
            printf '%s\t%s\t%s\t%s\n' "$name" "$t" "$(elapsed "$s" "$e")" "$result" | tee -a "$tsv"
        done
    fi
done <<<"$targets"

[ "$dry_run" -eq 1 ] || echo "timings: $tsv (test output: $log)"
