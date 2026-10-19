#!/usr/bin/env bash
# Fails if the public core API leaks SDK or media types: crates/core/src/api/
# and the other public modules (paths, secrets, logging) must not mention
# matrix_sdk, ruma, eyeball or imbl, nor the media crates (livekit, libwebrtc,
# webrtc-sys, cpal, sonora, nnnoiseless, tract, twicecat-audio; for video
# twicecat-video, ash, ashpd, pipewire, x11rb, yuvutils-rs), and crates/core/src/lib.rs
# must not `pub use` any of them. Fails closed: a missing path or grep error is
# an error.
set -euo pipefail
cd "$(dirname "$0")/.."
api_dir="crates/core/src/api"
lib_rs="crates/core/src/lib.rs"
# Media crate names are matched as whole words, so "contract" is not "tract".
crates='(matrix_sdk|matrix-sdk|ruma|eyeball|imbl|\blivekit|\blibwebrtc|\bwebrtc[_-]sys|\bcpal\b|\bsonora|\bnnnoiseless|\btract(_onnx|_core)?\b|\btwicecat[_-]audio\b|\btwicecat[_-]video\b|\bash\b|\bashpd\b|\bpipewire\b|\bx11rb\b|\byuvutils[_-]rs\b)'
[ -d "$api_dir" ] || { echo "error: $api_dir not found" >&2; exit 2; }
[ -f "$lib_rs" ] || { echo "error: $lib_rs not found" >&2; exit 2; }

status=0
# grep: 0 = match (violation), 1 = no match (ok), >1 = error (fail closed).
rc=0
grep -rnE "${crates}" "$api_dir" || rc=$?
if [ "$rc" -eq 0 ]; then
    echo "error: $api_dir must not mention SDK or media crates" >&2
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
    echo "error: $lib_rs must not re-export SDK or media crates" >&2
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
# `logging` exposes DEFAULT_FILTER, LogInitError, filter and init (tracing-subscriber
# setup, no SDK types) for the CLI and the app.
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

# Every allow-listed public module must be free of SDK crate names too, whether
# it is a file (`name.rs`) or a directory (`name/`). `api` is scanned above.
# Fails closed: each module must exist as a file or a directory.
for mod in paths secrets logging; do
    found=0
    for path in "crates/core/src/${mod}.rs" "crates/core/src/${mod}"; do
        [ -e "$path" ] || continue
        found=1
        rc=0
        grep -rnE "${crates}" "$path" || rc=$?
        if [ "$rc" -eq 0 ]; then
            echo "error: $path must not mention SDK or media crates" >&2
            status=1
        elif [ "$rc" -ne 1 ]; then
            echo "error: grep failed (status ${rc})" >&2
            exit "$rc"
        fi
    done
    [ "$found" -eq 1 ] || { echo "error: public module ${mod} not found" >&2; exit 2; }
done
exit "$status"
