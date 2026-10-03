#!/usr/bin/env bash
# Downloads NVIDIA's CUDA 13 and cuDNN 9 runtime libraries for the opt-in
# GPU speech enhancement (plan Task 13b) on Linux x86_64, without a system
# CUDA install or root: NVIDIA's own PyPI wheels at pinned versions, checked
# against the SHA-256 in scripts/cuda-runtime.sha256, their shared libraries
# unpacked into one cache dir, whose path is printed. Tessera preloads the
# libraries from there before ONNX Runtime's CUDA provider starts (or from
# TESSERA_CUDA_DIR). With CUDA 13 and cuDNN 9 installed system-wide this is
# not needed. The NVIDIA driver must be installed either way.
#
# The libraries are NVIDIA's, under NVIDIA's licences (copied next to them);
# about 1.1 GB downloaded, 1.8 GB unpacked.
#
# Usage: scripts/fetch-cuda-runtime.sh
# Env: TESSERA_CUDA_CACHE overrides the cache root.
# Progress goes to stderr; stdout carries only the path.
set -euo pipefail
cd "$(dirname "$0")/.."

# CUDA 13.0 runtime, cuBLAS 13.1, cuRAND 10.4, NVRTC 13.0, cuDNN 9.24: what
# ONNX Runtime 1.28's CUDA 13 build loads.
version=13.0
sums=scripts/cuda-runtime.sha256

die() { echo "error: $*" >&2; exit 1; }

sha256_of() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

[ "$(uname -s)" = Linux ] || die "Linux only (on Windows, install CUDA and cuDNN from NVIDIA)"
arch=$(uname -m)
[ "$arch" = x86_64 ] || die "no ONNX Runtime GPU build for $arch"

mapfile -t wheels < <(awk -v a="$arch" '!/^#/ && $2 == a {print $1 " " $3}' "$sums")
[ "${#wheels[@]}" -gt 0 ] || die "no pinned wheels for $arch in $sums"
pinned=$(printf '%s\n' "${wheels[@]}" | sha256sum | awk '{print $1}')

cache_root="${TESSERA_CUDA_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/tessera/cuda}"
dest="$cache_root/$version"
marker="$dest/.verified"

if [ ! -f "$marker" ] || [ "$(cat "$marker")" != "$pinned" ]; then
    mkdir -p "$cache_root"
    work=$(mktemp -d "$cache_root/.fetch.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    mkdir "$work/x" "$work/out" "$work/out/lib" "$work/out/licenses"
    for line in "${wheels[@]}"; do
        expected=${line%% *}
        url=${line#* }
        [[ "$expected" =~ ^[0-9a-f]{64}$ ]] || die "malformed SHA-256 in $sums"
        [[ "$url" == https://files.pythonhosted.org/* ]] || die "unexpected URL in $sums: $url"
        wheel=${url##*/}
        echo "downloading $wheel" >&2
        curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$work/$wheel" "$url"
        actual=$(sha256_of "$work/$wheel")
        [ "$actual" = "$expected" ] || die "SHA-256 mismatch for $wheel: expected $expected, got $actual"
        rm -rf "$work/x"/*
        if command -v unzip >/dev/null; then
            unzip -q "$work/$wheel" -d "$work/x"
        else
            python3 -m zipfile -e "$work/$wheel" "$work/x"
        fi
        rm "$work/$wheel"
        found=0
        for lib in "$work"/x/nvidia/*/lib/*.so*; do
            [ -f "$lib" ] || continue
            mv "$lib" "$work/out/lib/"
            found=1
        done
        [ "$found" = 1 ] || die "unexpected wheel layout (no nvidia/*/lib) in $wheel"
        for licence in "$work"/x/*.dist-info/licenses/* "$work"/x/*.dist-info/License.txt; do
            [ -f "$licence" ] && cp "$licence" "$work/out/licenses/${wheel%%-*}-$(basename "$licence")"
        done
    done
    printf '%s' "$pinned" >"$work/out/.verified"
    rm -rf "$dest"
    mv "$work/out" "$dest"
    echo "verified and unpacked into $dest" >&2
fi
printf '%s\n' "$dest/lib"
