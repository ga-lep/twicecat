#!/usr/bin/env bash
# Gives a release's files one naming scheme, so the release page groups them by
# system: twicecat-<tag>-<os>-<arch>.<format>, with the tag (v dropped, any
# label kept: 0.1.0-alpha), os one of linux, macos, windows, and arch x86_64 or
# arm64. Each packaging tool names its file its own way (amd64, aarch64, x64,
# Twicecat_, -1 package releases); this runs in the release job only, after
# the packages are built and smoke-tested, so nothing else sees these names.
# A file it does not know stops it, so a new format cannot keep an old name.
#
#   scripts/release-names.sh <dir> <tag>   renames the files in <dir>
#   scripts/release-names.sh --check       renames the current tools' names in a
#                                          scratch directory and compares
set -euo pipefail

die() { echo "error: $*" >&2; exit 1; }

# new_name <file> <tag version>: the file's name in the scheme, or nothing.
new_name() {
    local f=$1 v=$2 arch
    case "$f" in
        *aarch64* | *arm64*) arch=arm64 ;;
        *x86_64* | *amd64* | *x64*) arch=x86_64 ;;
        *) return 0 ;;
    esac
    case "$f" in
        *-symbols.tar.zst) echo "twicecat-$v-linux-$arch.debug-symbols.tar.zst" ;;
        *.tar.zst) echo "twicecat-$v-linux-$arch.tar.zst" ;;
        *.AppImage) echo "twicecat-$v-linux-$arch.AppImage" ;;
        *.AppImage.sig) echo "twicecat-$v-linux-$arch.AppImage.sig" ;;
        *.flatpak) echo "twicecat-$v-linux-$arch.flatpak" ;;
        *.deb) echo "twicecat-$v-linux-$arch.deb" ;;
        *.rpm) echo "twicecat-$v-linux-$arch.rpm" ;;
        *.dmg) echo "twicecat-$v-macos-$arch.dmg" ;;
        *.msi) echo "twicecat-$v-windows-$arch.msi" ;;
    esac
}

rename_all() {
    local dir=$1 tag=$2 v f new
    v=${tag#v}
    declare -A seen=()
    for path in "$dir"/*; do
        f=$(basename "$path")
        new=$(new_name "$f" "$v")
        [ -n "$new" ] || die "$f: no name for it in scripts/release-names.sh"
        [ -z "${seen[$new]:-}" ] || die "$f and ${seen[$new]} would both be $new"
        seen[$new]=$f
    done
    for path in "$dir"/*; do
        f=$(basename "$path")
        new=$(new_name "$f" "$v")
        [ "$f" = "$new" ] || mv -- "$path" "$dir/$new"
        echo "$f -> $new"
    done
}

if [ "${1:-}" = --check ]; then
    [ $# -eq 1 ] || die "usage: $0 --check"
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    # The names package.sh, flatpak.sh and cargo-packager gave v0.1.0-alpha.
    for f in twicecat-0.1.0-1.aarch64.rpm twicecat-0.1.0-1.x86_64.rpm \
        Twicecat-0.1.0-aarch64.AppImage Twicecat-0.1.0-x86_64.AppImage \
        Twicecat-0.1.0-aarch64.AppImage.sig Twicecat-0.1.0-x86_64.AppImage.sig \
        Twicecat-0.1.0-aarch64.flatpak Twicecat-0.1.0-x86_64.flatpak \
        twicecat-0.1.0-linux-aarch64-symbols.tar.zst twicecat-0.1.0-linux-aarch64.tar.zst \
        twicecat-0.1.0-linux-x86_64-symbols.tar.zst twicecat-0.1.0-linux-x86_64.tar.zst \
        twicecat_0.1.0-1_amd64.deb twicecat_0.1.0-1_arm64.deb \
        Twicecat_0.1.0_aarch64.dmg twicecat_0.1.0_x64_en-US.msi; do
        : >"$work/$f"
    done
    rename_all "$work" v0.1.0-alpha >/dev/null
    expected="twicecat-0.1.0-alpha-linux-arm64.AppImage
twicecat-0.1.0-alpha-linux-arm64.AppImage.sig
twicecat-0.1.0-alpha-linux-arm64.deb
twicecat-0.1.0-alpha-linux-arm64.debug-symbols.tar.zst
twicecat-0.1.0-alpha-linux-arm64.flatpak
twicecat-0.1.0-alpha-linux-arm64.rpm
twicecat-0.1.0-alpha-linux-arm64.tar.zst
twicecat-0.1.0-alpha-linux-x86_64.AppImage
twicecat-0.1.0-alpha-linux-x86_64.AppImage.sig
twicecat-0.1.0-alpha-linux-x86_64.deb
twicecat-0.1.0-alpha-linux-x86_64.debug-symbols.tar.zst
twicecat-0.1.0-alpha-linux-x86_64.flatpak
twicecat-0.1.0-alpha-linux-x86_64.rpm
twicecat-0.1.0-alpha-linux-x86_64.tar.zst
twicecat-0.1.0-alpha-macos-arm64.dmg
twicecat-0.1.0-alpha-windows-x86_64.msi"
    got=$(cd "$work" && LC_ALL=C ls)
    [ "$got" = "$expected" ] || { diff <(echo "$expected") <(echo "$got") >&2; die "unexpected names (above: expected, got)"; }
    # A file with no name in the scheme stops it.
    : >"$work/notes.txt"
    if (rename_all "$work" v0.1.0 >/dev/null 2>&1); then die "an unknown file was let through"; fi
    echo "ok: every current package name maps to the scheme, and an unknown file is refused"
    exit 0
fi

[ $# -eq 2 ] || die "usage: $0 <dir> <tag> | --check"
[ -d "$1" ] || die "no directory $1"
rename_all "$1" "$2"
