#!/usr/bin/env bash
# Checks that a release tag names the workspace's version: every crate takes
# its version from `[workspace.package]`, and the tag must be `v<version>`,
# optionally with a label (`v0.1.0-alpha`) that the packages do not carry:
# WiX takes only a numeric pre-release, so the label names the release, not
# the build. The release workflow makes a labelled tag a pre-release.
# The AppStream metadata must also list that version among its releases, so
# software centres show what changed. The release workflow runs it before
# building anything.
#
#   scripts/check-version.sh vX.Y.Z[-label]
set -euo pipefail
cd "$(dirname "$0")/.."

[ $# -eq 1 ] || { echo "usage: $0 vX.Y.Z[-label]" >&2; exit 2; }
tag=$1
[[ "$tag" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)(-[0-9A-Za-z][0-9A-Za-z.-]*)?$ ]] \
    || { echo "error: tag $tag is not vX.Y.Z or vX.Y.Z-label" >&2; exit 1; }
base=${BASH_REMATCH[1]}

# Capture first so a cargo failure aborts the script (set -e).
metadata=$(cargo metadata --no-deps --format-version 1 --locked)
versions=$(python3 -I -c '
import json, sys
for package in json.load(sys.stdin)["packages"]:
    print(package["name"], package["version"])
' <<<"$metadata")

status=0
while read -r name version; do
    if [ "$base" != "$version" ]; then
        echo "error: tag $tag does not match $name $version (expected v$version, optionally with a label)" >&2
        status=1
    fi
done <<<"$versions"
metainfo=packaging/linux/io.github.ga_lep.Twicecat.metainfo.xml
if ! grep -q "<release version=\"$base\"" "$metainfo"; then
    echo "error: $metainfo has no <release version=\"$base\"> entry" >&2
    status=1
fi
[ "$status" -eq 0 ] && echo "ok: $tag matches every crate and the metainfo"
exit "$status"
