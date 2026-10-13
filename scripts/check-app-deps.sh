#!/usr/bin/env bash
# Fails if a direct dependency of twicecat-app has a package name starting with
# the given regex (default: matrix-sdk, matrix_sdk, ruma, eyeball, imbl,
# twicecat-audio). The SDK's helper crates (ruma, eyeball, imbl) are refused as
# well, because they expose SDK types (ids, observables, vectors) and would let
# the app bypass the core's API boundary. twicecat-audio is refused because the
# app reaches calls and audio only through the core's commands and events,
# whose types are the core's own. Uses real package names, so renamed deps are caught
# too. Covers optional and target-specific deps too.
#
# The app also depends on twicecat-ptt (push-to-talk, started on the app's main
# thread). Its whole tree, not only its direct deps, must stay free of the SDK,
# the media crates and the core, so the app cannot reach them through it.
set -euo pipefail
pattern="${1:-(matrix-sdk|matrix_sdk|ruma|eyeball|imbl|twicecat-audio|twicecat_audio)}"
# Capture first so a cargo tree failure aborts the script (set -e).
deps=$(cargo tree -p twicecat-app --depth 1 -e normal,build,dev --prefix none --all-features --target all)
# tail skips the root package line. Plain assignment so set -e catches failures.
names=$(printf '%s\n' "$deps" | tail -n +2 | awk '{print $1}')
# grep: 0 = match (violation), 1 = no match (ok), >1 = error (fail closed).
rc=0
printf '%s\n' "$names" | grep -E "^${pattern}" || rc=$?
if [ "$rc" -eq 0 ]; then
    echo "error: twicecat-app must not depend directly on a package matching '^${pattern}'" >&2
    exit 1
elif [ "$rc" -ne 1 ]; then
    echo "error: grep failed (status ${rc})" >&2
    exit "$rc"
fi

ptt_pattern='(matrix-sdk|matrix_sdk|ruma|eyeball|imbl|twicecat-audio|twicecat_audio|twicecat-core|twicecat_core|livekit|libwebrtc|webrtc-sys)'
ptt_tree=$(cargo tree -p twicecat-ptt -e normal,build --prefix none --all-features --target all)
ptt_names=$(printf '%s\n' "$ptt_tree" | awk '{print $1}')
rc=0
# A prefix match, as above: ruma-common and livekit-api are caught too.
printf '%s\n' "$ptt_names" | grep -E "^${ptt_pattern}" || rc=$?
if [ "$rc" -eq 0 ]; then
    echo "error: twicecat-ptt must not depend, even indirectly, on a package matching '^${ptt_pattern}'" >&2
    exit 1
elif [ "$rc" -ne 1 ]; then
    echo "error: grep failed (status ${rc})" >&2
    exit "$rc"
fi
