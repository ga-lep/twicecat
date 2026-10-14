#!/usr/bin/env bash
# Fetches NVIDIA's CUDA driver API header (cuda.h) for webrtc-sys, which
# compiles LiveKit's NVENC encoder and NVDEC decoder only when it finds
# $CUDA_HOME/include/cuda.h at build time (webrtc-sys 0.3.49 build.rs:281-313;
# plan docs/plans/phase9-video.md Task 6). libcuda and libnvcuvid are loaded
# at run time, so the same binary still runs without an NVIDIA GPU.
#
# The header comes from NVIDIA's CUDA runtime wheel on PyPI, the one already
# pinned with its SHA-256 in scripts/checksums/cuda-runtime.sha256. Only
# cuda.h and the licence are kept, in the user cache; dev/cuda/include links
# to them and .cargo/config.toml points CUDA_HOME at dev/cuda. The header is
# NVIDIA's (licence copied next to it), used to build and never shipped.
# About 2 MB downloaded. Linux x86_64 only: webrtc-sys has no NVENC elsewhere
# that we build for.
#
# webrtc-sys's build script does not watch CUDA_HOME or the header, so a
# build made before this ran keeps no NVENC until `cargo clean -p webrtc-sys`.
#
# Usage: scripts/fetch-cuda-headers.sh
# Env: TWICECAT_CUDA_CACHE overrides the cache root.
# Progress goes to stderr; stdout carries only the CUDA_HOME path.
set -euo pipefail
cd "$(dirname "$0")/.."

sums=scripts/checksums/cuda-runtime.sha256

die() { echo "error: $*" >&2; exit 1; }

sha256_of() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

[ "$(uname -s)" = Linux ] && [ "$(uname -m)" = x86_64 ] ||
    die "LiveKit's NVENC builds only on Linux x86_64 here"

line=$(awk '!/^#/ && $2 == "x86_64" && $3 ~ /\/nvidia_cuda_runtime-/ {print $1 " " $3}' "$sums")
[ -n "$line" ] || die "no pinned nvidia_cuda_runtime wheel for x86_64 in $sums"
expected=${line%% *}
url=${line#* }
[[ "$expected" =~ ^[0-9a-f]{64}$ ]] || die "malformed SHA-256 in $sums"
[[ "$url" == https://files.pythonhosted.org/* ]] || die "unexpected URL in $sums: $url"
wheel=${url##*/}
version=$(echo "$wheel" | cut -d- -f2)

cache_root="${TWICECAT_CUDA_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/twicecat/cuda}"
dest="$cache_root/headers-$version"
marker="$dest/.verified"

if [ ! -f "$marker" ] || [ "$(cat "$marker")" != "$expected" ]; then
    mkdir -p "$cache_root"
    work=$(mktemp -d "$cache_root/.fetch.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    mkdir "$work/x" "$work/out" "$work/out/include" "$work/out/licenses"
    echo "downloading $wheel" >&2
    curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$work/$wheel" "$url"
    actual=$(sha256_of "$work/$wheel")
    [ "$actual" = "$expected" ] || die "SHA-256 mismatch for $wheel: expected $expected, got $actual"
    if command -v unzip >/dev/null; then
        unzip -q "$work/$wheel" -d "$work/x"
    else
        python3 -m zipfile -e "$work/$wheel" "$work/x"
    fi
    mapfile -t headers < <(find "$work/x/nvidia" -path '*/include/cuda.h' -type f)
    [ "${#headers[@]}" = 1 ] || die "unexpected wheel layout (${#headers[@]} include/cuda.h) in $wheel"
    mv "${headers[0]}" "$work/out/include/cuda.h"
    for licence in "$work"/x/*.dist-info/licenses/* "$work"/x/*.dist-info/License.txt; do
        [ -f "$licence" ] && cp "$licence" "$work/out/licenses/"
    done
    printf '%s' "$expected" >"$work/out/.verified"
    rm -rf "$dest"
    mv "$work/out" "$dest"
    echo "verified; cuda.h from $wheel in $dest" >&2
fi

link_dir=dev/cuda
[ -f "$link_dir/README.md" ] || die "$link_dir/README.md missing"
rm -f "$link_dir/include" "$link_dir/licenses"
ln -s "$dest/include" "$link_dir/include"
ln -s "$dest/licenses" "$link_dir/licenses"
printf '%s\n' "$dest"
