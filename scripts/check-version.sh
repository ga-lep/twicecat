#!/usr/bin/env bash
# Checks that a release tag names the workspace's version: every crate takes
# its version from `[workspace.package]`, and the tag must be `v<version>`.
# The release workflow runs it before building anything.
#
#   scripts/check-version.sh vX.Y.Z
set -euo pipefail
cd "$(dirname "$0")/.."

[ $# -eq 1 ] || { echo "usage: $0 vX.Y.Z" >&2; exit 2; }
tag=$1

# Capture first so a cargo failure aborts the script (set -e).
metadata=$(cargo metadata --no-deps --format-version 1 --locked)
versions=$(python3 -I -c '
import json, sys
for package in json.load(sys.stdin)["packages"]:
    print(package["name"], package["version"])
' <<<"$metadata")

status=0
while read -r name version; do
    if [ "$tag" != "v$version" ]; then
        echo "error: tag $tag does not match $name $version (expected v$version)" >&2
        status=1
    fi
done <<<"$versions"
[ "$status" -eq 0 ] && echo "ok: $tag matches every crate"
exit "$status"
