#!/usr/bin/env bash
# Fails if webrtc-sys downloaded libwebrtc itself during a build. Its
# downloader (webrtc-sys-build) creates <target>/<profile>/build/scratch-*/out/
# livekit_webrtc before fetching, and runs only when LK_CUSTOM_WEBRTC points
# at a missing directory; scripts/fetch-libwebrtc.sh is the only allowed source.
set -euo pipefail
cd "$(dirname "$0")/.."
target="${CARGO_TARGET_DIR:-target}"
[ -d "$target" ] || { echo "error: no build output in $target" >&2; exit 2; }
found=$(find "$target" -path '*/build/scratch-*/out/livekit_webrtc' -prune -print)
if [ -n "$found" ]; then
    printf 'error: webrtc-sys downloaded libwebrtc unverified:\n%s\n' "$found" >&2
    exit 1
fi
