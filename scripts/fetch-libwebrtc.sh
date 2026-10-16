#!/usr/bin/env bash
# Downloads LiveKit's prebuilt libwebrtc for the pinned tag, checks it against
# the SHA-256 in scripts/checksums/libwebrtc.sha256, unpacks it into a cache dir, and
# prints the directory to export as LK_CUSTOM_WEBRTC. webrtc-sys would
# otherwise download the same zip at build time without any checksum.
# For a host build it also links the unpacked entries into dev/libwebrtc, which
# .cargo/config.toml sets as LK_CUSTOM_WEBRTC (see dev/libwebrtc/README.md).
#
# Usage: scripts/fetch-libwebrtc.sh [triple]
#   triple: linux-x64 | linux-arm64 | mac-x64 | mac-arm64 | win-x64 | win-arm64
#           (default: the host)
# Env: TWICECAT_WEBRTC_CACHE overrides the cache root.
# Progress goes to stderr; stdout carries only the path.
set -euo pipefail
cd "$(dirname "$0")/.."

# Must match WEBRTC_TAG in the locked webrtc-sys-build (src/lib.rs).
tag=webrtc-89d790b
sums=scripts/checksums/libwebrtc.sha256

die() { echo "error: $*" >&2; exit 1; }

# fetch_to URL FILE: downloads URL into FILE, or, when TWICECAT_DOWNLOADS is
# set (an offline build such as the Flatpak), copies the file of the same
# name from that directory. The caller checks the SHA-256 either way.
fetch_to() {
    if [ -n "${TWICECAT_DOWNLOADS:-}" ]; then
        [ -f "$TWICECAT_DOWNLOADS/${1##*/}" ] || die "${1##*/} is not in TWICECAT_DOWNLOADS ($TWICECAT_DOWNLOADS)"
        cp "$TWICECAT_DOWNLOADS/${1##*/}" "$2"
        return
    fi
    echo "downloading $1" >&2
    curl -fsSL --proto '=https' --tlsv1.2 --retry 6 -o "$2" "$1"
}

host_triple() {
    local os arch
    case "$(uname -s)" in
        Linux) os=linux ;;
        Darwin) os=mac ;;
        MINGW* | MSYS* | CYGWIN*) os=win ;;
        *) die "unsupported OS: $(uname -s)" ;;
    esac
    case "$(uname -m)" in
        x86_64 | amd64) arch=x64 ;;
        aarch64 | arm64) arch=arm64 ;;
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

# A tool, preferring LLVM's (libwebrtc is built with clang) over binutils.
tool() {
    local t
    for t in "llvm-$1" "llvm-$1-22" "$1"; do
        if command -v "$t" >/dev/null; then
            printf '%s' "$t"
            return
        fi
    done
    die "need llvm-$1 or $1"
}

# libwebrtc's PipeWire screen capture (pipewire_stubs.o) defines weak pw_*
# functions that jump through pointers only its own dlopen loader fills.
# Linked statically next to libpipewire (cpal's PipeWire backend), they take
# the place of libpipewire's functions, and pw_init jumps to address 0.
# Renaming them, and the calls to them inside the archive, to
# twicecat_webrtc_pw_* keeps webrtc's capture working and frees the pw_* names.
# Checked by symbols, not by a pinned SHA-256: the patched bytes depend on the
# objcopy version, while the input zip is pinned above.
rename_pw_stubs() {
    local archive=$1 nm objcopy map renamed left stubs
    nm=$(tool nm)
    objcopy=$(tool objcopy)
    map=$(mktemp)
    "$nm" -A "$archive" 2>/dev/null |
        awk '$1 ~ /pipewire_stubs\.o:/ && $(NF-1) == "W" && $NF ~ /^pw_/ { print $NF, "twicecat_webrtc_" $NF }' |
        sort -u >"$map"
    renamed=$(wc -l <"$map")
    if [ "$renamed" -gt 0 ]; then
        "$objcopy" --redefine-syms="$map" "$archive"
    fi
    rm -f "$map"
    # Global symbols only: lowercase nm types are local (the AV1 assembly has
    # local pw_* constants, "packed words"), which cannot clash.
    left=$("$nm" -A "$archive" 2>/dev/null | awk '$(NF-1) ~ /^[A-Z]$/ && $NF ~ /^pw_/' | wc -l)
    [ "$left" -eq 0 ] || die "$left global pw_* symbols left in $archive after renaming"
    stubs=$("$nm" -A "$archive" 2>/dev/null |
        awk '$1 ~ /pipewire_stubs\.o:/ && $(NF-1) == "W" && $NF ~ /^twicecat_webrtc_pw_/' | wc -l)
    [ "$stubs" -gt 0 ] || die "no pw_* stubs found in $archive (unexpected libwebrtc layout)"
    echo "pw_* stubs renamed in $(basename "$archive"): $stubs (this run: $renamed)" >&2
}

unpack() {
    if command -v unzip >/dev/null; then
        unzip -q "$1" -d "$2"
    elif command -v python3 >/dev/null; then
        python3 -m zipfile -e "$1" "$2"
    else
        die "need unzip or python3"
    fi
}

triple="${1:-$(host_triple)}"
case "$triple" in
    linux-x64 | linux-arm64 | mac-x64 | mac-arm64 | win-x64 | win-arm64) ;;
    *) die "unknown triple: $triple" ;;
esac
name="webrtc-${triple}-release"
zip="${name}.zip"

expected=$(awk -v f="$zip" '$2 == f {print $1}' "$sums")
[ -n "$expected" ] || die "no pinned SHA-256 for $zip in $sums"
[[ "$expected" =~ ^[0-9a-f]{64}$ ]] || die "malformed SHA-256 for $zip in $sums"

cache_root="${TWICECAT_WEBRTC_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/twicecat/libwebrtc}"
dest="$cache_root/$tag/$triple"
lib_dir="$dest/${triple}-release"
marker="$dest/.verified"

if [ ! -f "$marker" ] || [ "$(cat "$marker")" != "$expected" ]; then
    mkdir -p "$cache_root/$tag"
    work=$(mktemp -d "$cache_root/$tag/.fetch.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    url="https://github.com/livekit/rust-sdks/releases/download/${tag}/${zip}"
    # GitHub's release downloads answer 5xx now and then (a 504 failed CI);
    # six retries back off over about a minute.
    fetch_to "$url" "$work/$zip"
    actual=$(sha256_of "$work/$zip")
    if [ "$actual" != "$expected" ]; then
        die "SHA-256 mismatch for $zip: expected $expected, got $actual"
    fi
    mkdir "$work/out"
    unpack "$work/$zip" "$work/out"
    [ -f "$work/out/${triple}-release/webrtc.ninja" ] \
        || die "unexpected archive layout (no ${triple}-release/webrtc.ninja)"
    if [ "${triple%%-*}" = linux ]; then
        rename_pw_stubs "$work/out/${triple}-release/lib/libwebrtc.a"
        : >"$work/out/.pw-renamed"
    fi
    printf '%s' "$expected" >"$work/out/.verified"
    rm -rf "$dest"
    mv "$work/out" "$dest"
    echo "verified and unpacked into $dest" >&2
fi

# A cache verified before the rename existed gets it now.
if [ "${triple%%-*}" = linux ] && [ ! -f "$dest/.pw-renamed" ]; then
    rename_pw_stubs "$lib_dir/lib/libwebrtc.a"
    : >"$dest/.pw-renamed"
fi

# Windows symlinks need extra privileges; there, export LK_CUSTOM_WEBRTC
# (it takes precedence over .cargo/config.toml).
if [ -z "${1:-}" ] && [ "${triple%%-*}" != win ]; then
    link_dir=dev/libwebrtc
    [ -f "$link_dir/README.md" ] || die "$link_dir/README.md missing"
    find "$link_dir" -mindepth 1 -maxdepth 1 -type l -delete
    for entry in "$lib_dir"/*; do
        ln -s "$entry" "$link_dir/$(basename "$entry")"
    done
fi

if [ "${triple%%-*}" = win ] && command -v cygpath >/dev/null; then
    cygpath -w "$lib_dir"
else
    printf '%s\n' "$lib_dir"
fi
