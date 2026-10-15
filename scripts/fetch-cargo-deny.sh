#!/usr/bin/env bash
# Downloads the pinned cargo-deny release (EmbarkStudios/cargo-deny, MIT OR
# Apache-2.0) for x86-64 Linux, checks it against the SHA-256 in
# scripts/checksums/cargo-deny.sha256, unpacks it into a cache dir, and prints
# the path of the binary. CI used the Docker-based cargo-deny-action, which took
# about a minute to build and run; this is a 5 MB download.
#
# Usage: scripts/fetch-cargo-deny.sh     then:  "$(scripts/fetch-cargo-deny.sh)" check
# Env: TWICECAT_TOOLS_CACHE overrides the cache root.
# Progress goes to stderr; stdout carries only the path.
#
# The checksum is the one GitHub computed for the release asset (the "digest" of
# the release API), which also matches the project's own .sha256 file. To move to
# a new release, change `version` and the checksum together.
set -euo pipefail
cd "$(dirname "$0")/.."

version=0.20.2
archive="cargo-deny-${version}-x86_64-unknown-linux-musl.tar.gz"
sums=scripts/checksums/cargo-deny.sha256

die() { echo "error: $*" >&2; exit 1; }

[ "$(uname -s)-$(uname -m)" = Linux-x86_64 ] \
    || die "only x86-64 Linux is supported here; elsewhere: cargo install cargo-deny --locked --version $version"

expected=$(awk -v f="$archive" '$2 == f {print $1}' "$sums")
[ -n "$expected" ] || die "no pinned SHA-256 for $archive in $sums"
[[ "$expected" =~ ^[0-9a-f]{64}$ ]] || die "malformed SHA-256 for $archive in $sums"

cache_root="${TWICECAT_TOOLS_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/twicecat/tools}"
dest="$cache_root/cargo-deny-$version"
marker="$dest/.verified"

if [ ! -f "$marker" ] || [ "$(cat "$marker")" != "$expected" ]; then
    mkdir -p "$cache_root"
    work=$(mktemp -d "$cache_root/.fetch.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    url="https://github.com/EmbarkStudios/cargo-deny/releases/download/${version}/${archive}"
    echo "downloading $url" >&2
    curl -fsSL --proto '=https' --tlsv1.2 --retry 6 -o "$work/$archive" "$url"
    actual=$(sha256sum "$work/$archive" | awk '{print $1}')
    [ "$actual" = "$expected" ] || die "SHA-256 mismatch for $archive: expected $expected, got $actual"
    mkdir "$work/x" "$work/out"
    tar -xzf "$work/$archive" -C "$work/x"
    bin=$(find "$work/x" -type f -name cargo-deny | head -n 1)
    [ -n "$bin" ] || die "unexpected archive layout (no cargo-deny binary)"
    mv "$bin" "$work/out/cargo-deny"
    chmod +x "$work/out/cargo-deny"
    printf '%s' "$expected" >"$work/out/.verified"
    rm -rf "$dest"
    mv "$work/out" "$dest"
    echo "verified and unpacked into $dest" >&2
fi
printf '%s\n' "$dest/cargo-deny"
