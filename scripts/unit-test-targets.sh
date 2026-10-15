#!/usr/bin/env bash
# The cargo arguments of the `test` job's run: everything `cargo test --workspace`
# builds except the integration test files whose tests are all #[ignore]
# (they need the local homeserver and run in the integration shards). Each of
# those files is a test binary of about 500 MB that statically links matrix-sdk
# and libwebrtc; linking 26 of them for "0 passed; N ignored" was most of the
# link time and disk writes of every OS job.
#
#   scripts/unit-test-targets.sh          prints: --lib --bins --examples --test a --test b ...
#   scripts/unit-test-targets.sh --skipped  prints the skipped files, one per line
#
# A file is skipped only when it has #[ignore] attributes at least as many as
# test attributes (#[test], #[tokio::test], ...); a doubt keeps the file. The doc
# tests are not selectable together with these arguments: run
# `cargo test --workspace --doc` as well.
set -euo pipefail
cd "$(dirname "$0")/.."

# 0 = every test of the file is ignored.
all_ignored() {
    local tests ignores
    tests=$(grep -cE '^[[:space:]]*#\[([A-Za-z_]+::)*test([](]|[[:space:]]*$)' "$1" || true)
    ignores=$(grep -cE '^[[:space:]]*#\[ignore' "$1" || true)
    [ "$tests" -gt 0 ] && [ "$ignores" -ge "$tests" ]
}

mode=${1:-}
args=(--lib --bins --examples)
for f in crates/*/tests/*.rs; do
    name=$(basename "$f" .rs)
    if all_ignored "$f"; then
        [ "$mode" = --skipped ] && echo "$f"
    else
        args+=(--test "$name")
    fi
done
[ "$mode" = --skipped ] || echo "${args[*]}"
