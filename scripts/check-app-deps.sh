#!/usr/bin/env bash
# Fails if a direct dependency of tessera-app has a package name starting with
# the given pattern (default: matrix-sdk). Uses real package names, so renamed
# deps are caught too.
set -euo pipefail
pattern="${1:-matrix-sdk}"
# tail skips the root package line.
if cargo tree -p tessera-app --depth 1 -e normal,build,dev --prefix none | tail -n +2 | awk '{print $1}' | grep -E "^${pattern}"; then
    echo "error: tessera-app must not depend directly on '${pattern}*'" >&2
    exit 1
fi
