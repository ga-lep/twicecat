#!/usr/bin/env bash
# Speed reference for the hardware encoders on this machine (Phase 9 Task 3):
# FFmpeg encodes 30 s of a 4K60 test clip with each encoder that uses the
# driver path one of our backends will take, and the result is a table of
# throughput, FFmpeg's time per encode call (p50/p99), CPU use and bitrate
# accuracy. FFmpeg is a measuring instrument here, never a dependency.
#
# Encoders tried (each skipped when FFmpeg or the driver lacks it):
#   nvenc-h264, nvenc-av1   NVENC on the NVIDIA GPU (CUDA path)
#   vulkan-h264@<gpu>       Vulkan Video on each Vulkan GPU; Intel's Mesa
#                           driver needs ANV_DEBUG=video-encode, set here
#   vaapi-h264@<node>       VA-API on each render node (dropped from the
#                           plan, measured for comparison)
#
# The clip: 2 s of FFmpeg's testsrc2 (moving content) at 3840x2160 NV12,
# 1.5 GB of raw frames in a temporary directory (RAM on most systems),
# looped to 30 s. Input frames come from CPU memory, as on our CPU-copy
# path; capture to encoder without a copy is not measured here.
#
# Times: FFmpeg encodes on its own thread and the hardware encoders keep
# a few frames in flight, so the per-call time is an upper bound on what
# the encoder adds per frame, not its latency. Throughput (frames per second
# of wall time, no pacing) is the number that says whether 4K60 fits.
#
# Usage: scripts/measure-encoders.sh [seconds (30)] [out.tsv]
# Env: TWICECAT_MEASURE_DIR (where the clip and outputs go; default mktemp)
set -euo pipefail

seconds="${1:-30}"
out_tsv="${2:-}"
width=3840
height=2160
fps=60
frames=$((seconds * fps))

command -v ffmpeg >/dev/null || { echo "error: ffmpeg not found" >&2; exit 2; }
dir="${TWICECAT_MEASURE_DIR:-$(mktemp -d)}"
cleanup() { [ -z "${TWICECAT_MEASURE_DIR:-}" ] && rm -rf "$dir"; }
trap cleanup EXIT

clip="$dir/clip.nv12"
if [ ! -s "$clip" ]; then
    echo "making the test clip (2 s of 4K60 NV12)..." >&2
    ffmpeg -hide_banner -loglevel error -f lavfi \
        -i "testsrc2=size=${width}x${height}:rate=${fps}" -t 2 \
        -pix_fmt nv12 -f rawvideo "$clip"
fi
input=(-stream_loop -1 -f rawvideo -pix_fmt nv12 -s "${width}x${height}" -r "$fps" -i "$clip" -t "$seconds")

encoders=$(ffmpeg -hide_banner -encoders 2>/dev/null)
has() { grep -qE "^ V.{5} $1 " <<<"$encoders"; }

rows=()
# Set by `run`: 0 when the encoder ran, 1 when it failed (its row is then
# added only when `quiet` is empty).
ran=0
quiet=""
# run NAME TARGET_BPS ENV... -- FFMPEG ARGS (before the input) -- (after it)
run() {
    local name=$1 target=$2
    shift 2
    local envs=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    local pre=()
    while [ "$1" != "--" ]; do pre+=("$1"); shift; done
    shift
    local log="$dir/$name.log" stream="$dir/$name.mkv"
    echo "measuring $name..." >&2
    if ! env "${envs[@]}" timeout -k 10 300 ffmpeg -nostdin -hide_banner -nostats -benchmark -benchmark_all \
        "${pre[@]}" "${input[@]}" "$@" -an -f matroska -y "$stream" 2>"$log" ||
        [ ! -s "$stream" ]; then
        ran=1
        [ -n "$quiet" ] && return
        rows+=("$name	failed	-	-	-	-	-	$(grep -m1 -iE 'unable|unsupported|not supported|error|fail' "$log" | sed 's/^\[[^]]*\] *//' | tr '\t' ' ' | cut -c1-90)")
        return
    fi
    ran=0
    local bytes
    bytes=$(stat -c %s "$stream")
    # Per-call times (µs) of encode_video, and the final summary line.
    local p50 p99 rtime cpu
    read -r p50 p99 < <(awk '/bench:.* encode_video /{for(i=1;i<=NF;i++) if($(i+1)=="real"){print $i}}' "$log" |
        sort -n | awk '{a[NR]=$1} END {
            if (NR == 0) { print "- -"; exit }
            hi = int(NR * 0.99); if (hi < 1) hi = 1
            printf "%.2f %.2f\n", a[int(NR * 0.5) + 1] / 1000, a[hi] / 1000
        }')
    read -r rtime cpu < <(awk -F'[= ]+' '/bench: utime=/{u=$3; s=$5; r=$7; gsub("s","",u); gsub("s","",s); gsub("s","",r); printf "%s %.0f\n", r, (u+s)/r*100}' "$log")
    local throughput bitrate accuracy
    throughput=$(awk -v f="$frames" -v r="$rtime" 'BEGIN {printf "%.1f", f/r}')
    bitrate=$(awk -v b="$bytes" -v s="$seconds" 'BEGIN {printf "%.2f", b*8/s/1e6}')
    accuracy=$(awk -v b="$bytes" -v s="$seconds" -v t="$target" 'BEGIN {printf "%+.1f", (b*8/s/t-1)*100}')
    rows+=("$name	ok	$throughput	$p50	$p99	$cpu	$bitrate	${accuracy}% of $((target / 1000000)) Mb/s")
}

# NVENC (Q6: NVIDIA Ada and newer). Constrained Baseline, CBR, no B-frames,
# low latency, as our encoder will be set up.
if has h264_nvenc; then
    run nvenc-h264 20000000 -- -- -c:v h264_nvenc -preset p1 -tune ull -profile:v baseline \
        -rc cbr -b:v 20M -maxrate 20M -bufsize 20M -bf 0 -g 600
fi
if has av1_nvenc; then
    run nvenc-av1 12000000 -- -- -c:v av1_nvenc -preset p1 -tune ull \
        -rc cbr -b:v 12M -maxrate 12M -bufsize 12M -bf 0 -g 600
fi

# Vulkan Video on each GPU Vulkan lists (Q6: Intel, AMD, NVIDIA before Ada).
# Our settings first (Constrained Baseline, CBR); where FFmpeg or the driver
# refuses them, the closest settings that run, named in the row: High
# profile with VBR, then a constant quantizer (no rate control at all). The
# other encoders put a keyframe every 10 s.
if has h264_vulkan && command -v vulkaninfo >/dev/null; then
    index=0
    while read -r gpu; do
        tag=$(tr -c 'A-Za-z0-9' '-' <<<"$gpu" | tr -s '-' | sed 's/-$//' | cut -c1-28)
        # One keyframe for the whole run: FFmpeg 9.0.2's h264_vulkan
        # deadlocks at the second IDR on NVIDIA's 615.78 driver.
        common=(-init_hw_device "vulkan=vk:$index" -filter_hw_device vk --
            -vf hwupload -c:v h264_vulkan -bf 0 -g 1000000)
        quiet=1
        run "vulkan-h264@$tag" 20000000 ANV_DEBUG=video-encode RADV_PERFTEST=video_encode -- \
            "${common[@]}" -profile:v constrained_baseline -rc_mode cbr -b:v 20M -maxrate 20M -bufsize 20M
        if [ "$ran" -ne 0 ]; then
            run "vulkan-h264@$tag (High, VBR)" 20000000 ANV_DEBUG=video-encode RADV_PERFTEST=video_encode -- \
                "${common[@]}" -profile:v high -rc_mode vbr -b:v 20M -maxrate 20M -bufsize 20M
        fi
        if [ "$ran" -ne 0 ]; then
            quiet=""
            run "vulkan-h264@$tag (CQP 26, no rate control)" 20000000 ANV_DEBUG=video-encode RADV_PERFTEST=video_encode -- \
                "${common[@]}" -profile:v constrained_baseline -rc_mode cqp -qp 26
        fi
        quiet=""
        index=$((index + 1))
    done < <(vulkaninfo --summary 2>/dev/null | sed -n 's/^\s*deviceName\s*=\s*//p')
fi

# VA-API on each render node (dropped by the human, measured for comparison).
if has h264_vaapi; then
    for node in /dev/dri/renderD*; do
        run "vaapi-h264@$(basename "$node")" 20000000 -- -vaapi_device "$node" -- \
            -vf format=nv12,hwupload -c:v h264_vaapi -profile:v constrained_baseline \
            -rc_mode CBR -b:v 20M -maxrate 20M -bufsize 20M -bf 0 -g 600
    done
fi

header="encoder	result	fps (unpaced)	call p50 ms	call p99 ms	CPU %	Mb/s	bitrate"
{
    echo "$header"
    printf '%s\n' "${rows[@]}"
} | column -t -s $'\t'
if [ -n "$out_tsv" ]; then
    { echo "$header"; printf '%s\n' "${rows[@]}"; } >"$out_tsv"
fi
