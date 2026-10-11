#!/usr/bin/env bash
# Measures the whole call path per 10 ms frame against the Steam Frame's CPU
# budget (5 % of a core: 500 us per 10 ms; phase 6 plan Task 15), for each
# noise-suppression mode.
#
#   scripts/profile-call-path.sh [--voices N] [--frames N] [--cpu C]
#
# Call path = the processing chain (voice_profile's "chain:" row for the mode)
# + the voice engine's cost of mixing N remote voices (its "engine: default, N
# voices mixed" row less the one with none) + Opus encode at 64 kb/s,
# complexity 9 (opus_profile) + N Opus decodes. Two scenarios per mode: people
# talking (voice in noise) and digital silence (muted, gate closed). Medians
# and p99s of the parts are added (an upper bound on the sum's own figures).
# A row over the budget is marked OVER; that is a report, not an error. A
# failing profile run is an error. Nothing is downloaded: DeepFilterNet3 is
# measured only when scripts/fetch-models.sh has already fetched its models.
# --cpu C pins the runs to CPU C with taskset (run it on an E-core too). The
# report is also appended to $GITHUB_STEP_SUMMARY when that is set.
set -euo pipefail
cd "$(dirname "$0")/.."

voices=4
frames=1000
cpu=""
budget_us=500

usage() {
    echo "usage: $0 [--voices N] [--frames N] [--cpu C]" >&2
    exit 2
}
# number N: N is 1 to 9 decimal digits (leading zeros allowed); it is read as
# decimal, never octal (see "10#" below).
number() { [[ "$1" =~ ^[0-9]{1,9}$ ]] || usage; }
while [ $# -gt 0 ]; do
    case "$1" in
        --voices) [ $# -ge 2 ] || usage; number "$2"; voices=$((10#$2)); shift 2 ;;
        --frames) [ $# -ge 2 ] || usage; number "$2"; frames=$((10#$2)); shift 2 ;;
        --cpu) [ $# -ge 2 ] || usage; number "$2"; cpu=$((10#$2)); shift 2 ;;
        *) usage ;;
    esac
done
[ "$frames" -ge 100 ] || { echo "error: --frames must be at least 100" >&2; exit 2; }

echo "building the profile examples (release, --locked)..." >&2
cargo build --release --locked -q -p twicecat-audio --example voice_profile
cargo build --release --locked -q -p twicecat-core --example opus_profile
examples="${CARGO_TARGET_DIR:-target}/release/examples"
voice_profile="$examples/voice_profile"
opus_profile="$examples/opus_profile"
if [ ! -x "$voice_profile" ] || [ ! -x "$opus_profile" ]; then
    echo "error: examples not found in $examples" >&2
    exit 1
fi

pinned() {
    if [ -n "$cpu" ]; then
        taskset -c "$cpu" "$@"
    else
        "$@"
    fi
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# column NAME FILE COL: the figure in column COL (2 mean, 3 median, 4 p99) of
# the row NAME; empty when the row is absent.
column() {
    awk -F'\t' -v n="$1" -v c="$3" '$1 == n { print $c; exit }' "$2"
}

# profile ROW: runs voice_profile on the rows whose name contains ROW (frames
# frames); the TSV goes to $work/voice.tsv (appended).
profile() {
    pinned "$voice_profile" --tsv "$1" "$frames" >>"$work/voice.tsv"
}

: >"$work/voice.tsv"
profile "chain: nothing"
profile "chain: WebRTC NS"
profile "chain: RNNoise"
if ! profile "chain: DeepFilterNet3 (CPU)" 2>"$work/dfn.err"; then
    cat "$work/dfn.err" >&2
    echo "error: voice_profile failed on the DeepFilterNet3 row" >&2
    exit 1
fi
profile "engine: default, 0 voices mixed"
# The N-voice row is added by --voices when it is not one of the usual ones.
pinned "$voice_profile" --tsv --voices "$voices" "engine: default, $voices voices mixed" "$frames" \
    >>"$work/voice.tsv"
seconds=$((frames / 100))
[ "$seconds" -ge 2 ] || seconds=2
pinned "$opus_profile" --tsv "$seconds" >"$work/opus.tsv"

base=$(column "engine: default, 0 voices mixed" "$work/voice.tsv" 3)
base99=$(column "engine: default, 0 voices mixed" "$work/voice.tsv" 4)
mix=$(column "engine: default, $voices voices mixed" "$work/voice.tsv" 3)
mix99=$(column "engine: default, $voices voices mixed" "$work/voice.tsv" 4)
[ -n "$base" ] && [ -n "$mix" ] || { echo "error: no engine row" >&2; exit 1; }

report="$work/report"
{
    echo "Call path per 10 ms frame, $voices remote voices, $frames frames, 64 kb/s Opus complexity 9"
    echo "Budget: 5 % of a core = ${budget_us} us per 10 ms"
    echo
    echo "Host: $(uname -m), $(awk -F': *' '/^(model name|Model|Hardware)/ { print $2; exit }' /proc/cpuinfo 2>/dev/null || true)"
    echo "Pinned to CPU: ${cpu:-no}"
    echo "Denormals: flushed on the processing thread (voice_profile calls flush_denormals_on_this_thread as the engine does); Opus rows are unflushed, as on WebRTC's encoder thread"
    echo
    printf '%-22s %-9s %9s %7s %9s %7s\n' "mode" "scenario" "median us" "budget" "p99 us" "budget"
    for entry in "off|chain: nothing (gate, limiter)" \
        "WebRTC NS|chain: WebRTC NS (high)" \
        "RNNoise|chain: RNNoise" \
        "DeepFilterNet3 CPU|chain: DeepFilterNet3 (CPU)"; do
        mode=${entry%%|*}
        row=${entry#*|}
        chain=$(column "$row" "$work/voice.tsv" 3)
        chain99=$(column "$row" "$work/voice.tsv" 4)
        if [ -z "$chain" ]; then
            if [ "$mode" = "DeepFilterNet3 CPU" ]; then
                printf '%-22s skipped (no row; models fetched? scripts/fetch-models.sh)\n' "$mode"
                if [ -s "$work/dfn.err" ]; then
                    echo "  voice_profile said:"
                    sed 's/^/    /' "$work/dfn.err"
                fi
                continue
            fi
            echo "error: no voice_profile row '$row'" >&2
            exit 1
        fi
        for scenario in "voice in noise|talking" "digital silence|silent"; do
            signal=${scenario%%|*}
            label=${scenario#*|}
            enc=$(column "encode 64 kb/s c9 $signal unflushed" "$work/opus.tsv" 3)
            enc99=$(column "encode 64 kb/s c9 $signal unflushed" "$work/opus.tsv" 4)
            dec=$(column "decode 64 kb/s c9 $signal unflushed" "$work/opus.tsv" 3)
            dec99=$(column "decode 64 kb/s c9 $signal unflushed" "$work/opus.tsv" 4)
            [ -n "$enc" ] && [ -n "$dec" ] || { echo "error: no opus row for $signal" >&2; exit 1; }
            awk -v mode="$mode" -v label="$label" -v budget="$budget_us" -v n="$voices" \
                -v chain="$chain" -v chain99="$chain99" -v mix="$mix" -v base="$base" \
                -v mix99="$mix99" -v base99="$base99" -v enc="$enc" -v enc99="$enc99" \
                -v dec="$dec" -v dec99="$dec99" 'BEGIN {
                    m = mix - base; if (m < 0) m = 0
                    m99 = mix99 - base99; if (m99 < 0) m99 = 0
                    med = chain + m + enc + n * dec
                    p99 = chain99 + m99 + enc99 + n * dec99
                    flag = (med > budget || p99 > budget) ? "  OVER" : ""
                    printf "%-22s %-9s %9.1f %6.1f%% %9.1f %6.1f%%%s\n", mode, label, med, 100 * med / budget, p99, 100 * p99 / budget, flag
                }'
        done
    done
    echo
    echo "Parts (median / p99 us per 10 ms): voice engine mixing $voices voices $(awk -v a="$mix" -v b="$base" -v c="$mix99" -v d="$base99" 'BEGIN { printf "%.1f / %.1f", (a > b ? a - b : 0), (c > d ? c - d : 0) }'); Opus rows from opus_profile (encode, decode per voice) below"
    grep -E '^(encode|decode) 64 kb/s c9 (voice in noise|digital silence) unflushed' "$work/opus.tsv" | awk -F'\t' '{ printf "  %-48s %8.1f / %8.1f\n", $1, $3, $4 }'
    echo
    echo "Chain rows (median / p99 us):"
    awk -F'\t' '$1 ~ /^chain:/ { printf "  %-48s %8.1f / %8.1f\n", $1, $3, $4 }' "$work/voice.tsv"
} >"$report"

cat "$report"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
        echo '### Call path against the Frame budget'
        echo '```'
        cat "$report"
        echo '```'
    } >>"$GITHUB_STEP_SUMMARY"
fi
