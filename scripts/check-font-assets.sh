#!/usr/bin/env bash
# Checks the bundled Inter files against the SHA-256 list in SOURCE.md.
# Downloads nothing.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dir="$root/crates/app/assets/fonts/inter"
source_md="$dir/SOURCE.md"
list=$(sed -n '/^```sha256$/,/^```$/p' "$source_md" | grep -E '^[0-9a-f]{64}  ')
count=$(printf '%s\n' "$list" | wc -l)
if [ "$count" -ne 5 ]; then
  echo "expected 5 entries in $source_md, found $count" >&2
  exit 1
fi
names=$(printf '%s\n' "$list" | awk '{print $2}' | sort | tr '\n' ' ')
expected="Inter-Bold.ttf Inter-Italic.ttf Inter-Regular.ttf Inter-SemiBold.ttf OFL.txt "
if [ "$names" != "$expected" ]; then
  echo "unexpected file names in $source_md: $names" >&2
  exit 1
fi
cd "$dir"
printf '%s\n' "$list" | sha256sum -c --quiet -
echo "OK for $count files"
