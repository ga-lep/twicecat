#!/usr/bin/env bash
# Downloads Microsoft's prebuilt ONNX Runtime at the pinned version for the
# opt-in GPU speech enhancement (plan Task 13b; tessera-audio feature
# `onnxruntime`), checks the SHA-256 in scripts/onnxruntime.sha256 (GitHub
# release digests), unpacks it into a cache dir, and prints the directory
# holding the shared library. Tessera loads it from there at run time (or
# from TESSERA_ONNXRUNTIME_DIR); nothing is linked at build time.
#
# Usage: scripts/fetch-onnxruntime.sh [variant] [platform]
#   variant:  cpu (default) | cuda12 | cuda13   (CUDA: linux-x64, win-x64;
#             the GPU archives are 240-450 MB and need the NVIDIA CUDA and
#             cuDNN runtime libraries installed)
#   platform: linux-x64 | linux-aarch64 | osx-arm64 | win-x64 | win-arm64
#             (default: the host)
# Env: TESSERA_ONNXRUNTIME_CACHE overrides the cache root.
# Progress goes to stderr; stdout carries only the path.
set -euo pipefail
cd "$(dirname "$0")/.."

# Matches the API level of the locked ort crate (2.0.0-rc.13, ONNX Runtime 1.28).
version=1.28.3
sums=scripts/onnxruntime.sha256

die() { echo "error: $*" >&2; exit 1; }

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

sha256_of() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "need sha256sum or shasum"
    fi
}

variant="${1:-cpu}"
platform="${2:-$(host_platform)}"
case "$variant" in
    cpu) flavour="" ;;
    cuda12 | cuda13)
        case "$platform" in
            linux-x64 | win-x64) flavour="-gpu_${variant}" ;;
            *) die "no $variant build for $platform" ;;
        esac
        ;;
    *) die "unknown variant: $variant" ;;
esac
case "$platform" in
    linux-x64 | linux-aarch64 | osx-arm64) ext=tgz ;;
    win-x64 | win-arm64) ext=zip ;;
    *) die "unknown platform: $platform" ;;
esac
name="onnxruntime-${platform}${flavour}-${version}"
archive="${name}.${ext}"

expected=$(awk -v f="$archive" '$2 == f {print $1}' "$sums")
[ -n "$expected" ] || die "no pinned SHA-256 for $archive in $sums"
[[ "$expected" =~ ^[0-9a-f]{64}$ ]] || die "malformed SHA-256 for $archive in $sums"

cache_root="${TESSERA_ONNXRUNTIME_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/tessera/onnxruntime}"
dest="$cache_root/$version/${platform}${flavour}"
marker="$dest/.verified"

if [ ! -f "$marker" ] || [ "$(cat "$marker")" != "$expected" ]; then
    mkdir -p "$cache_root/$version"
    work=$(mktemp -d "$cache_root/$version/.fetch.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    url="https://github.com/microsoft/onnxruntime/releases/download/v${version}/${archive}"
    echo "downloading $url" >&2
    curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$work/$archive" "$url"
    actual=$(sha256_of "$work/$archive")
    [ "$actual" = "$expected" ] || die "SHA-256 mismatch for $archive: expected $expected, got $actual"
    mkdir "$work/x"
    if [ "$ext" = tgz ]; then
        tar -xzf "$work/$archive" -C "$work/x"
    elif command -v unzip >/dev/null; then
        unzip -q "$work/$archive" -d "$work/x"
    else
        python3 -m zipfile -e "$work/$archive" "$work/x"
    fi
    [ -d "$work/x/$name/lib" ] || die "unexpected archive layout (no $name/lib)"
    mv "$work/x/$name" "$work/out"
    printf '%s' "$expected" >"$work/out/.verified"
    rm -rf "$dest"
    mv "$work/out" "$dest"
    echo "verified and unpacked into $dest" >&2
fi
printf '%s\n' "$dest/lib"
