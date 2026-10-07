#!/usr/bin/env bash
# Downloads the DeepFilterNet3 ONNX models (Rikorose/DeepFilterNet v0.5.6,
# MIT OR Apache-2.0) at a pinned commit, checks the SHA-256 in
# scripts/checksums/models.sha256, unpacks them into a cache dir, and prints that
# directory. tessera-audio looks for the models there (or in
# TESSERA_MODEL_DIR); without them DeepFilterNet falls back to RNNoise.
#
# Usage: scripts/fetch-models.sh
# Env: TESSERA_MODEL_CACHE overrides the cache root.
# Progress goes to stderr; stdout carries only the path.
set -euo pipefail
cd "$(dirname "$0")/.."

# DeepFilterNet's v0.5.6 tag.
commit=978576aa8400552a4ce9730838c635aa30db5e61
archive=DeepFilterNet3_onnx.tar.gz
sums=scripts/checksums/models.sha256

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

expected=$(awk -v f="$archive" '$2 == f {print $1}' "$sums")
[ -n "$expected" ] || die "no pinned SHA-256 for $archive in $sums"
[[ "$expected" =~ ^[0-9a-f]{64}$ ]] || die "malformed SHA-256 for $archive in $sums"

cache_root="${TESSERA_MODEL_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/tessera/models}"
dest="$cache_root/deepfilternet3"
marker="$dest/.verified"

if [ ! -f "$marker" ] || [ "$(cat "$marker")" != "$expected" ]; then
    mkdir -p "$cache_root"
    work=$(mktemp -d "$cache_root/.fetch.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    url="https://github.com/Rikorose/DeepFilterNet/raw/${commit}/models/${archive}"
    echo "downloading $url" >&2
    curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$work/$archive" "$url"
    actual=$(sha256_of "$work/$archive")
    [ "$actual" = "$expected" ] || die "SHA-256 mismatch for $archive: expected $expected, got $actual"
    mkdir "$work/x" "$work/out"
    tar -xzf "$work/$archive" -C "$work/x"
    for f in enc.onnx erb_dec.onnx df_dec.onnx config.ini; do
        [ -f "$work/x/tmp/export/$f" ] || die "unexpected archive layout (no tmp/export/$f)"
        mv "$work/x/tmp/export/$f" "$work/out/$f"
    done
    cat >"$work/out/SOURCE" <<SRC
DeepFilterNet3 ONNX export from https://github.com/Rikorose/DeepFilterNet
commit $commit (tag v0.5.6), models/$archive, SHA-256 $expected.
Licence: MIT OR Apache-2.0 (the repository's LICENSE-MIT and LICENSE-APACHE).
SRC
    printf '%s' "$expected" >"$work/out/.verified"
    rm -rf "$dest"
    mv "$work/out" "$dest"
    echo "verified and unpacked into $dest" >&2
fi
printf '%s\n' "$dest"
