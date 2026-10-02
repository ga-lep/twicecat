#!/usr/bin/env bash
# Checks the commits of a feature branch (AGENTS.md rule 5): every commit in
# base..head is a Conventional Commit of type feat or docs; no merges and no
# leftover fixup!/squash!/amend! commits (fold them with
# `git rebase --autosquash <base>`).
#
# Usage: scripts/check-commits.sh <base> [head]   (head defaults to HEAD)
set -euo pipefail
base="${1:?usage: check-commits.sh <base> [head]}"
head="${2:-HEAD}"
# Capture first so a git failure aborts the script (set -e).
merges=$(git rev-list --merges "$base..$head")
log=$(git log --format='%h %s' --no-merges "$base..$head")

status=0
if [ -n "$merges" ]; then
    printf 'error: merge commits on the branch (rebase instead):\n%s\n' "$merges" >&2
    status=1
fi
pattern='^[0-9a-f]+ (feat|docs)(\([a-z0-9._/-]+\))?!?: [^ ].*$'
while IFS= read -r line; do
    [ -n "$line" ] || continue
    if [[ "$line" =~ ^[0-9a-f]+\ (fixup|squash|amend)!\  ]]; then
        echo "error: unfolded commit: $line" >&2
        status=1
    elif ! [[ "$line" =~ $pattern ]]; then
        echo "error: not a feat/docs Conventional Commit: $line" >&2
        status=1
    fi
done <<<"$log"
exit "$status"
