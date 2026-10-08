#!/usr/bin/env bash
# Downloads NVIDIA's TensorRT 10 runtime for the GPU speech enhancement's
# TensorRT backend (plan Task 13b), on Linux x86_64, without root: NVIDIA's
# own wheel at a pinned version, checked against the SHA-256 in
# scripts/checksums/tensorrt.sha256. Only what ONNX Runtime's TensorRT provider loads
# is unpacked: the runtime, its ONNX parser and plugins, and the engine
# builder's resources for this machine's GPU (from nvidia-smi; all Linux
# ones when it is missing) plus the portable PTX ones. Tessera preloads them
# from the printed directory (or TESSERA_TENSORRT_DIR).
# Needs scripts/fetch-cuda-runtime.sh (or a system CUDA 13 + cuDNN 9).
#
# The libraries are NVIDIA's, under NVIDIA's licence (copied next to them);
# the wheel is 3.7 GB, about 1.2 GB stays unpacked for one GPU.
#
# Usage: scripts/fetch-tensorrt.sh
# Env: TESSERA_CUDA_CACHE overrides the cache root (as fetch-cuda-runtime.sh).
# Progress goes to stderr; stdout carries only the path.
set -euo pipefail
cd "$(dirname "$0")/.."

version=10.16.1
sums=scripts/checksums/tensorrt.sha256

die() { echo "error: $*" >&2; exit 1; }

sha256_of() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

[ "$(uname -s)" = Linux ] || die "Linux only (on Windows, install TensorRT from NVIDIA)"
arch=$(uname -m)
[ "$arch" = x86_64 ] || die "no ONNX Runtime GPU build for $arch"
read -r expected url < <(awk -v a="$arch" '!/^#/ && $2 == a {print $1, $3}' "$sums")
[[ "$expected" =~ ^[0-9a-f]{64}$ ]] || die "no pinned SHA-256 for $arch in $sums"
[[ "$url" == https://pypi.nvidia.com/* ]] || die "unexpected URL in $sums: $url"

# The GPU architectures to keep builder resources for (sm120 for compute
# capability 12.0); every Linux one without nvidia-smi.
archs=$(nvidia-smi --query-gpu=compute_cap --format=csv,noheader 2>/dev/null | tr -d . | sort -u | sed 's/^/sm/' | tr '\n' ' ' || true)
pinned="$expected ${archs:-all}"

cache_root="${TESSERA_CUDA_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/tessera/cuda}"
dest="$cache_root/tensorrt-$version"
marker="$dest/.verified"

if [ ! -f "$marker" ] || [ "$(cat "$marker")" != "$pinned" ]; then
    mkdir -p "$cache_root"
    work=$(mktemp -d "$cache_root/.fetch.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    wheel=${url##*/}
    echo "downloading $wheel (3.7 GB)" >&2
    curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$work/$wheel" "$url"
    actual=$(sha256_of "$work/$wheel")
    [ "$actual" = "$expected" ] || die "SHA-256 mismatch for $wheel: expected $expected, got $actual"
    members=(libnvinfer.so.10 libnvinfer_plugin.so.10 libnvonnxparser.so.10 "libnvinfer_builder_resource_ptx.so.$version")
    if [ -n "$archs" ]; then
        for sm in $archs; do
            members+=("libnvinfer_builder_resource_$sm.so.$version")
        done
    else
        while read -r name; do
            members+=("$name")
        done < <(python3 -m zipfile -l "$work/$wheel" | awk '{print $1}' |
            sed -n 's|^tensorrt_libs/\(libnvinfer_builder_resource_sm[0-9]*\.so\..*\)|\1|p')
    fi
    mkdir -p "$work/out/lib" "$work/out/licenses"
    python3 - "$work/$wheel" "$work/out" "${members[@]}" <<'PY'
import sys, zipfile
wheel, out, *names = sys.argv[1:]
z = zipfile.ZipFile(wheel)
listed = set(z.namelist())
for name in names:
    member = f"tensorrt_libs/{name}"
    if member not in listed:
        sys.exit(f"error: {member} not in the wheel (no builder for this GPU?)")
    with z.open(member) as src, open(f"{out}/lib/{name}", "wb") as dst:
        while chunk := src.read(1 << 24):
            dst.write(chunk)
for member in listed:
    if member.endswith("dist-info/LICENSE.txt"):
        with open(f"{out}/licenses/tensorrt-LICENSE.txt", "wb") as dst:
            dst.write(z.read(member))
PY
    printf '%s' "$pinned" >"$work/out/.verified"
    rm -rf "$dest"
    mv "$work/out" "$dest"
    echo "verified and unpacked into $dest" >&2
fi
printf '%s\n' "$dest/lib"
