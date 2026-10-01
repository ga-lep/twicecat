#!/usr/bin/env bash
# Fails if the public core API leaks SDK types: crates/core/src/api/ must not
# mention matrix_sdk, ruma, eyeball or imbl, and crates/core/src/lib.rs must not
# `pub use` any of them. Fails closed: a missing path or grep error is an error.
set -euo pipefail
cd "$(dirname "$0")/.."
api_dir="crates/core/src/api"
lib_rs="crates/core/src/lib.rs"
crates='(matrix_sdk|matrix-sdk|ruma|eyeball|imbl)'
[ -d "$api_dir" ] || { echo "error: $api_dir not found" >&2; exit 2; }
[ -f "$lib_rs" ] || { echo "error: $lib_rs not found" >&2; exit 2; }

status=0
# grep: 0 = match (violation), 1 = no match (ok), >1 = error (fail closed).
rc=0
grep -rnE "${crates}" "$api_dir" || rc=$?
if [ "$rc" -eq 0 ]; then
    echo "error: $api_dir must not mention SDK crates" >&2
    status=1
elif [ "$rc" -ne 1 ]; then
    echo "error: grep failed (status ${rc})" >&2
    exit "$rc"
fi

rc=0
grep -nE "^[[:space:]]*pub[[:space:]]+(\([a-z]+\)[[:space:]]+)?(use|extern[[:space:]]+crate)[^;]*${crates}" "$lib_rs" || rc=$?
if [ "$rc" -eq 0 ]; then
    echo "error: $lib_rs must not re-export SDK crates" >&2
    status=1
elif [ "$rc" -ne 1 ]; then
    echo "error: grep failed (status ${rc})" >&2
    exit "$rc"
fi
exit "$status"
