#!/usr/bin/env bash
# Downloads ONNX Runtime's WebGPU plugin execution provider for the GPU
# speech enhancement on GPUs that CUDA and TensorRT do not reach (plan Task
# 13b part 4: AMD, Intel, the Steam Deck; Vulkan on Linux, Direct3D 12 on
# Windows, Metal on macOS): Microsoft's wheel at a pinned version, checked
# against the SHA-256 in scripts/checksums/webgpu-ep.sha256. Only the library (Dawn
# inside, one file), its licence and its third-party notices are unpacked.
# Tessera registers it with ONNX Runtime from the printed directory (or
# TESSERA_WEBGPU_EP_DIR). Needs scripts/fetch-onnxruntime.sh (1.24.4 or
# later) and, on Linux, a Vulkan loader (libvulkan.so.1) and driver.
#
# Usage: scripts/fetch-webgpu-ep.sh [platform]
#   platform: linux-x64 | linux-aarch64 | win-x64 | win-arm64 | osx-arm64
#             (default: the host)
# Env: TESSERA_ONNXRUNTIME_CACHE overrides the cache root (as
# fetch-onnxruntime.sh). Progress goes to stderr; stdout carries only the path.
set -euo pipefail
cd "$(dirname "$0")/.."

version=0.4.0
sums=scripts/checksums/webgpu-ep.sha256

die() { echo "error: $*" >&2; exit 1; }

sha256_of() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

host_platform() {
    local os arch
    case "$(uname -s)" in
        Linux) os=linux ;;
        Darwin) os=osx ;;
        MINGW* | MSYS* | CYGWIN*) os=win ;;
        *) die "unsupported OS: $(uname -s)" ;;
    esac
    case "$(uname -m)" in
        x86_64 | amd64) arch=x64 ;;
        aarch64 | arm64) arch=$([ "$os" = linux ] && echo aarch64 || echo arm64) ;;
        *) die "unsupported architecture: $(uname -m)" ;;
    esac
    printf '%s-%s' "$os" "$arch"
}

platform=${1:-$(host_platform)}
read -r expected url < <(awk -v p="$platform" '!/^#/ && $2 == p {print $1, $3}' "$sums")
[[ "${expected:-}" =~ ^[0-9a-f]{64}$ ]] || die "no pinned SHA-256 for $platform in $sums"
[[ "$url" == https://files.pythonhosted.org/* ]] || die "unexpected URL in $sums: $url"
case "$platform" in
    win-*) library=onnxruntime_providers_webgpu.dll ;;
    osx-*) library=libonnxruntime_providers_webgpu.dylib ;;
    *) library=libonnxruntime_providers_webgpu.so ;;
esac

cache_root="${TESSERA_ONNXRUNTIME_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/tessera/onnxruntime}"
dest="$cache_root/webgpu-ep-$version"
marker="$dest/.verified"

if [ ! -f "$marker" ] || [ "$(cat "$marker")" != "$expected" ]; then
    mkdir -p "$cache_root"
    work=$(mktemp -d "$cache_root/.fetch.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    wheel=${url##*/}
    echo "downloading $wheel" >&2
    curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$work/$wheel" "$url"
    actual=$(sha256_of "$work/$wheel")
    [ "$actual" = "$expected" ] || die "SHA-256 mismatch for $wheel: expected $expected, got $actual"
    mkdir -p "$work/out/licenses"
    python3 - "$work/$wheel" "$work/out" "$library" <<'PY'
import sys, zipfile
wheel, out, library = sys.argv[1:]
z = zipfile.ZipFile(wheel)
names = z.namelist()
member = f"onnxruntime_ep_webgpu/{library}"
if member not in names:
    sys.exit(f"error: {member} not in the wheel")
with z.open(member) as src, open(f"{out}/{library}", "wb") as dst:
    while chunk := src.read(1 << 24):
        dst.write(chunk)
for name in names:
    for kept in ("LICENSE", "ThirdPartyNotices.txt"):
        if name.endswith(f"dist-info/licenses/{kept}"):
            with open(f"{out}/licenses/{kept}", "wb") as dst:
                dst.write(z.read(name))
PY
    printf '%s' "$expected" >"$work/out/.verified"
    rm -rf "$dest"
    mv "$work/out" "$dest"
    echo "verified and unpacked into $dest" >&2
fi
printf '%s\n' "$dest"
