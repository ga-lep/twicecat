#!/usr/bin/env bash
# Measures what the CPU level and the build profile change, so the choice of
# release builds rests on numbers (Phase 7 plan, "Measurements"): per variant,
# the build time, the size of `twicecat` (as built, stripped, and stripped and
# compressed), and the voice chain per 10 ms frame (voice_profile's "chain:"
# rows) on the chosen cores, over interleaved rounds.
#
#   scripts/measure-builds.sh [--variants "v1 v3"] [--profile release|dist]
#                             [--cores "2 12"] [--rounds N] [--frames N]
#                             [--no-build]
#
# A variant is a `target-cpu`: v1 (x86-64), v2, v3, v4 (x86-64-vN), native, or
# any name `rustc --print target-cpus` lists (aarch64: generic, cortex-a76,
# neoverse-n2, cortex-a720, ...). Defaults: on x86_64 v1 and v3, plus v4 when
# this CPU has AVX-512; on aarch64 generic, cortex-a76 (Armv8.2 with dot
# product and FP16: the Steam Frame, a Raspberry Pi 5 and the CI runner all
# run it) and native. `--cores` defaults to the second CPU (the first takes
# most interrupts), and on hybrid Intel CPUs also to the second E-core.
#
# Each variant is built with `--target <host>` and its target-cpu appended to
# the config's flags (`--config`, so `.cargo/config.toml`'s linker and CRT
# flags stay): flags then reach only the code under test, never the build
# scripts, which run here (a v4 build script dies with SIGILL on a CPU without
# AVX-512). A variant this CPU cannot run is built and sized, not timed.
# Builds go to target/variants/<profile>-<variant>. The report is Markdown on
# stdout, also appended to $GITHUB_STEP_SUMMARY when that is set; progress
# goes to stderr. Downloads nothing; DeepFilterNet3 rows only when
# scripts/fetch-models.sh has fetched the models.
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
    echo "usage: $0 [--variants \"v1 v3\"] [--profile release|dist] [--cores \"2 12\"] [--rounds N] [--frames N] [--no-build]" >&2
    exit 2
}
# number N: 1 to 9 decimal digits, read as decimal (see "10#" below).
number() { [[ "$1" =~ ^[0-9]{1,9}$ ]] || usage; }

variants=""
profile=release
cores=""
rounds=3
frames=3000
build=1
while [ $# -gt 0 ]; do
    case "$1" in
        --variants) [ $# -ge 2 ] || usage; variants=$2; shift 2 ;;
        --profile) [ $# -ge 2 ] || usage; profile=$2; shift 2 ;;
        --cores) [ $# -ge 2 ] || usage; cores=$2; shift 2 ;;
        --rounds) [ $# -ge 2 ] || usage; number "$2"; rounds=$((10#$2)); shift 2 ;;
        --frames) [ $# -ge 2 ] || usage; number "$2"; frames=$((10#$2)); shift 2 ;;
        --no-build) build=0; shift ;;
        *) usage ;;
    esac
done
case "$profile" in release | dist) ;; *) usage ;; esac
[ "$rounds" -ge 1 ] || usage
[ "$frames" -ge 100 ] || { echo "error: --frames must be at least 100" >&2; exit 2; }

host=$(rustc -vV | awk '/^host:/ { print $2 }')
arch=${host%%-*}
if [ -z "$variants" ]; then
    case "$arch" in
        x86_64)
            variants="v1 v3"
            grep -qw avx512f /proc/cpuinfo 2>/dev/null && variants="$variants v4"
            ;;
        aarch64) variants="generic cortex-a76 native" ;;
        *) echo "error: no default variants for $arch; pass --variants" >&2; exit 2 ;;
    esac
fi
if [ -z "$cores" ]; then
    cores=0
    [ "$(nproc)" -gt 1 ] && cores=1
    # Hybrid Intel CPUs list their efficiency cores here.
    if [ -r /sys/devices/cpu_atom/cpus ]; then
        e_core=$(cut -d- -f1 </sys/devices/cpu_atom/cpus | cut -d, -f1)
        [ -n "$e_core" ] && cores="$cores $((e_core + 1))"
    fi
fi
for c in $cores; do number "$c"; done

# cpu_name VARIANT: the target-cpu it stands for.
cpu_name() {
    case "$1" in
        v1) echo x86-64 ;;
        v2 | v3 | v4) echo "x86-64-$1" ;;
        *) echo "$1" ;;
    esac
}
for v in $variants; do
    [[ "$v" =~ ^[a-z0-9._-]+$ ]] || { echo "error: bad variant name: $v" >&2; exit 2; }
    cpu=$(cpu_name "$v")
    if [ "$cpu" != native ] && ! rustc --print target-cpus --target "$host" | awk '{ print $1 }' | grep -qx -- "$cpu"; then
        echo "error: rustc does not know the CPU $cpu for $host" >&2
        exit 2
    fi
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

dir_of() { echo "target/variants/$profile-$1"; }
out_of() { echo "$(dir_of "$1")/$host/$profile"; }

: >"$work/builds.tsv"
for v in $variants; do
    cpu=$(cpu_name "$v")
    seconds=-
    if [ "$build" -eq 1 ]; then
        echo "building $v (target-cpu=$cpu, profile $profile)..." >&2
        start=$(date +%s)
        for what in "-p twicecat-app --bin twicecat" "-p twicecat-audio --example voice_profile"; do
            # shellcheck disable=SC2086 # $what is two words on purpose
            CARGO_TARGET_DIR=$(dir_of "$v") cargo build --locked -q --profile "$profile" \
                --target "$host" --config "target.$host.rustflags = ['-C', 'target-cpu=$cpu']" $what
        done
        seconds=$(($(date +%s) - start))
    fi
    app="$(out_of "$v")/twicecat"
    [ -f "$app" ] || { echo "error: $app not built (drop --no-build)" >&2; exit 1; }
    strip -o "$work/$v.stripped" "$app"
    raw=$(stat -c %s "$app")
    stripped=$(stat -c %s "$work/$v.stripped")
    packed=$(zstd -19 -T0 -q -c "$work/$v.stripped" | wc -c)
    # Can this CPU run it? (SIGILL is status 132.)
    runs=yes
    "$(out_of "$v")/examples/voice_profile" --tsv "chain: nothing" 100 >/dev/null 2>&1 || runs=no
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$v" "$seconds" "$raw" "$stripped" "$packed" "$runs" >>"$work/builds.tsv"
done

# Timing: rounds of every runnable variant on every core, interleaved so that
# heat and clock changes spread over all variants.
: >"$work/chain.tsv"
for round in $(seq 1 "$rounds"); do
    for core in $cores; do
        while IFS=$'\t' read -r v _ _ _ _ runs; do
            [ "$runs" = yes ] || continue
            echo "round $round/$rounds, core $core, $v..." >&2
            taskset -c "$core" "$(out_of "$v")/examples/voice_profile" --tsv chain "$frames" </dev/null 2>/dev/null |
                awk -F'\t' -v v="$v" -v c="$core" 'BEGIN { OFS = "\t" } { print v, c, $1, $3 }' >>"$work/chain.tsv"
        done <"$work/builds.tsv"
    done
done

report() {
    local model
    model=$(awk -F': ' '/^model name/ { print $2; exit }' /proc/cpuinfo 2>/dev/null || true)
    echo "### Builds: $host, profile \`$profile\`"
    echo
    echo "CPU: ${model:-unknown}. Rust $(rustc -V | awk '{ print $2 }'). Chain: median µs per 10 ms frame, median of $rounds rounds of $frames frames (±: half the spread of the rounds)."
    echo
    echo "| Variant | Build | \`twicecat\` | stripped | stripped, zstd -19 | runs here |"
    echo "| --- | ---: | ---: | ---: | ---: | --- |"
    awk -F'\t' '{
        build = ($2 == "-") ? "-" : sprintf("%d min %02d s", $2 / 60, $2 % 60)
        printf "| %s | %s | %.1f MB | %.1f MB | %.1f MB | %s |\n", $1, build, $3 / 1e6, $4 / 1e6, $5 / 1e6, $6
    }' "$work/builds.tsv"
    [ -s "$work/chain.tsv" ] || return 0
    python3 -I - "$work/chain.tsv" "$work/builds.tsv" <<'EOF'
import statistics, sys
rows, variants, cores, data = [], [], [], {}
for line in open(sys.argv[2]):
    v = line.split("\t")[0]
    if line.rstrip("\n").split("\t")[5] == "yes":
        variants.append(v)
for line in open(sys.argv[1]):
    v, core, row, median = line.rstrip("\n").split("\t")
    if row not in rows:
        rows.append(row)
    if core not in cores:
        cores.append(core)
    data.setdefault((core, row, v), []).append(float(median))
for core in cores:
    print()
    print(f"Chain on CPU {core}:")
    print()
    print("| Row | " + " | ".join(variants) + " |")
    print("| --- |" + " ---: |" * len(variants))
    for row in rows:
        cells = []
        for v in variants:
            x = data.get((core, row, v))
            cells.append(f"{statistics.median(x):.1f} ±{(max(x) - min(x)) / 2:.1f}" if x else "-")
        print(f"| {row} | " + " | ".join(cells) + " |")
EOF
}

report
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    report >>"$GITHUB_STEP_SUMMARY"
fi
