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

# `-z` treats the whole file as one record, so `[^;]*` spans lines and a
# multi-line `pub use matrix_sdk::{\n    Client,\n};` is caught.
rc=0
grep -Pzo "pub\s*(\(\s*[a-z]+\s*\)\s*)?(use|extern\s+crate)[^;]*${crates}" "$lib_rs" >/dev/null || rc=$?
if [ "$rc" -eq 0 ]; then
    echo "error: $lib_rs must not re-export SDK crates" >&2
    status=1
elif [ "$rc" -ne 1 ]; then
    echo "error: grep failed (status ${rc})" >&2
    exit "$rc"
fi

# api/ may re-export only from api/ itself: no `pub use crate::<other>`,
# `pub use super::super`, or `pub use ::`, which could leak engine types.
rc=0
grep -rPzo --include='*.rs' "pub\s*(\(\s*[a-z]+\s*\)\s*)?use\s+(crate\s*::\s*(?!api\b)|super\s*::\s*super|::)" "$api_dir" >/dev/null || rc=$?
if [ "$rc" -eq 0 ]; then
    echo "error: $api_dir must not re-export anything from outside api/" >&2
    status=1
elif [ "$rc" -ne 1 ]; then
    echo "error: grep failed (status ${rc})" >&2
    exit "$rc"
fi

# lib.rs may declare only these public modules. The engine is private and
# exported item by item, so no SDK type can leak through a module path.
# `logging` exposes only `init` (tracing-subscriber setup, no SDK types) for the
# CLI and the app.
allowed='(api|paths|secrets|logging)'
rc=0
bad=$(grep -Pzo "pub\s*(\(\s*[a-z]+\s*\)\s*)?mod\s+(?!${allowed}\s*[;{])\w+" "$lib_rs" | tr '\0' '\n') || rc=$?
if [ "$rc" -eq 0 ]; then
    echo "error: $lib_rs declares a pub mod outside the allow-list (api, paths, secrets, logging):" >&2
    echo "$bad" >&2
    status=1
elif [ "$rc" -ne 1 ]; then
    echo "error: grep failed (status ${rc})" >&2
    exit "$rc"
fi
exit "$status"
