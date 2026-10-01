#!/usr/bin/env bash
# Fails if a direct dependency of tessera-app has a package name starting with
# the given pattern (default: matrix-sdk). Uses real package names, so renamed
# deps are caught too. Covers optional and target-specific deps too.
set -euo pipefail
pattern="${1:-matrix-sdk}"
# Capture first so a cargo tree failure aborts the script (set -e).
deps=$(cargo tree -p tessera-app --depth 1 -e normal,build,dev --prefix none --all-features --target all)
# tail skips the root package line. Plain assignment so set -e catches failures.
names=$(printf '%s\n' "$deps" | tail -n +2 | awk '{print $1}')
# grep: 0 = match (violation), 1 = no match (ok), >1 = error (fail closed).
rc=0
printf '%s\n' "$names" | grep -E "^${pattern}" || rc=$?
if [ "$rc" -eq 0 ]; then
    echo "error: tessera-app must not depend directly on '${pattern}*'" >&2
    exit 1
elif [ "$rc" -ne 1 ]; then
    echo "error: grep failed (status ${rc})" >&2
    exit "$rc"
fi
