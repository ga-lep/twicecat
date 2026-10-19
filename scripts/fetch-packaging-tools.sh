#!/usr/bin/env bash
# Downloads the Linux packaging tools at pinned versions, checks the SHA-256
# in scripts/checksums/packaging-tools.sha256, installs them into a cache dir,
# and prints that directory: `nfpm`, `appimagetool` and the AppImage `runtime`
# for this machine's architecture. scripts/package.sh uses them.
#
# Usage: scripts/fetch-packaging-tools.sh
# Env: TWICECAT_TOOLS_CACHE overrides the cache root.
# Progress goes to stderr; stdout carries only the path.
set -euo pipefail
cd "$(dirname "$0")/.."

sums=scripts/checksums/packaging-tools.sha256

die() { echo "error: $*" >&2; exit 1; }

sha256_of() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "need sha256sum or shasum"
    fi
}

arch=$(uname -m)
case "$arch" in
    x86_64 | aarch64) ;;
    arm64) arch=aarch64 ;;
    *) die "no packaging tools pinned for $arch" ;;
esac

# The pinned set's identity: when the pins change, the tools are fetched again.
pins=$(grep -v '^#' "$sums" | awk -v a="$arch" '$2 == a')
[ "$(wc -l <<<"$pins")" -eq 3 ] || die "expected 3 tools for $arch in $sums"
identity=$(printf '%s\n' "$pins" | sha256sum | awk '{print $1}')

cache_root="${TWICECAT_TOOLS_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/twicecat/tools}"
dest="$cache_root/packaging-$arch"
marker="$dest/.verified"

if [ ! -f "$marker" ] || [ "$(cat "$marker")" != "$identity" ]; then
    mkdir -p "$cache_root"
    work=$(mktemp -d "$cache_root/.fetch.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    mkdir "$work/out"
    while read -r expected _ tool url; do
        [[ "$expected" =~ ^[0-9a-f]{64}$ ]] || die "malformed SHA-256 for $tool in $sums"
        [[ "$url" == https://github.com/* ]] || die "unexpected URL for $tool: $url"
        echo "downloading $url" >&2
        curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$work/$tool.download" "$url"
        actual=$(sha256_of "$work/$tool.download")
        [ "$actual" = "$expected" ] || die "SHA-256 mismatch for $tool: expected $expected, got $actual"
        case "$tool" in
            nfpm)
                mkdir "$work/nfpm"
                tar -xzf "$work/nfpm.download" -C "$work/nfpm"
                [ -f "$work/nfpm/nfpm" ] || die "unexpected nfpm archive layout"
                install -m 755 "$work/nfpm/nfpm" "$work/out/nfpm"
                ;;
            appimagetool) install -m 755 "$work/appimagetool.download" "$work/out/appimagetool" ;;
            runtime) install -m 644 "$work/runtime.download" "$work/out/runtime" ;;
            *) die "unknown tool $tool in $sums" ;;
        esac
    done <<<"$pins"
    printf '%s' "$identity" >"$work/out/.verified"
    rm -rf "$dest"
    mv "$work/out" "$dest"
    echo "verified and installed into $dest" >&2
fi
printf '%s\n' "$dest"
