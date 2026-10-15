#!/usr/bin/env bash
# Turns the performance indicators of a run into a Markdown table.
#
#   scripts/perf-report.sh CURRENT.jsonl [BASELINE.jsonl]
#
# CURRENT is the file `twicecat-perf` appended to ($TWICECAT_PERF_FILE): one JSON
# object per line with name, value, unit, expect and ok. BASELINE, when given,
# is the same file of an earlier run (the last successful one on main); the
# table then shows its value and the change. A name recorded twice counts with
# its last value. The table goes to stdout as Markdown. When $GITHUB_STEP_SUMMARY
# is set it is appended there as Markdown (the run's Summary page renders it) and
# stdout gets aligned plain text, which a step's log can show.
#
# This is a report, not a check (docs/testing.md, rule R3): it exits 0 whatever
# the numbers are, and fails only when a file cannot be read. Needs jq.
set -euo pipefail

current=${1:?usage: perf-report.sh CURRENT.jsonl [BASELINE.jsonl]}
baseline=${2:-/dev/null}
[ -r "$current" ] || { echo "error: cannot read $current" >&2; exit 1; }
[ -r "$baseline" ] || { echo "error: cannot read $baseline" >&2; exit 1; }

report=$(jq -rn --slurpfile cur "$current" --slurpfile base "$baseline" '
  def last_by_name(xs): reduce xs[] as $m ({}; .[$m.name] = $m);
  def num: if . == null then "n/a" else (. * 100 | round / 100 | tostring) end;
  def expected: if . == null then "report only"
    elif .op == "min" then "at least \(.value | num)"
    else "at most \(.value | num)" end;
  last_by_name($cur) as $c
  | last_by_name($base) as $b
  | ($c | length) as $n
  | ([$c[] | select(.ok | not)] | length) as $outside
  | "### Performance indicators",
    "",
    "\($n) recorded, \($outside) outside their expectation. Indicators only: this report never fails a build.",
    "",
    "| Indicator | Value | Expected | Last main | Change |",
    "|---|---:|---|---:|---:|",
    ($c | to_entries | sort_by(.key)[]
      | .value as $m
      | ($b[.key] // null) as $p
      | "| `\(.key)` | \($m.value | num) \($m.unit)\(if $m.ok then "" else " ⚠️" end) | \($m.expect | expected) | "
        + (if $p == null then "" else "\($p.value | num) \($p.unit)" end) + " | "
        + (if $p != null and $p.value != null and $m.value != null and $p.value != 0
           then (($m.value - $p.value) / $p.value * 100 | round) as $d
                | (if $d > 0 then "+\($d)%" else "\($d)%" end)
           else "" end)
        + " |")
')
# The step summary renders Markdown, the log of a step does not: with a summary
# file the log gets the same table as aligned plain text.
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    printf '%s\n' "$report" >> "$GITHUB_STEP_SUMMARY"
    if command -v column >/dev/null; then
        printf '%s\n' "$report" | sed -e '/^|---/d' -e 's/`//g' -e 's/ ⚠️/ (outside)/' -e 's/^### //' \
            -e '/^| /{s/^| //;s/ |$//;s/ | /\t/g}' |
            awk -F'\t' 'NF > 1 { print | "column -t -s \"$(printf \"\\t\")\""; next } { close("column -t -s \"$(printf \"\\t\")\""); print }'
        exit 0
    fi
fi
printf '%s\n' "$report"
