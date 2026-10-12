#!/usr/bin/env bash
# Clears the debug builds when they grow past a limit.
#
# Cargo never deletes an older build: every new mix of features, of packages
# picked with -p, or of dependencies gets a full set of its own beside the
# others (about 27 GB of test binaries here), so target/debug only grows. This
# removes target/debug (`cargo clean --profile dev`) once it is over the limit;
# release builds stay. The next debug build starts from scratch (a few
# minutes).
#
# Usage: scripts/trim-target.sh [limit in GB]   (default 80, or
# TWICECAT_TARGET_LIMIT_GB). Honours CARGO_TARGET_DIR.
set -euo pipefail

limit_gb=${1:-${TWICECAT_TARGET_LIMIT_GB:-80}}
if ! [[ $limit_gb =~ ^[0-9]+$ ]]; then
    echo "usage: $0 [limit in GB]" >&2
    exit 2
fi
root=$(cd "$(dirname "$0")/.." && pwd)
debug="${CARGO_TARGET_DIR:-$root/target}/debug"
if [ ! -d "$debug" ]; then
    exit 0
fi
size_kb=$(du -sk "$debug" | cut -f1)
size_gb=$((size_kb / 1024 / 1024))
if [ "$size_kb" -le $((limit_gb * 1024 * 1024)) ]; then
    echo "target/debug: ${size_gb} GB, under the ${limit_gb} GB limit"
    exit 0
fi
echo "target/debug: ${size_gb} GB, over the ${limit_gb} GB limit; removing it"
cargo clean --profile dev --manifest-path "$root/Cargo.toml"
