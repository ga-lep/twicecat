#!/usr/bin/env bash
# Headless smoke test of the Linux packages scripts/package.sh made (Phase 7
# plan Task 6): each must install where it is meant to, resolve every library
# the binary links, carry the resources in the layout the app reads, and
# print `twicecat <version>` for `--version`.
#
#   scripts/smoke-test-packages.sh <package dir>
#
# The .tar.zst and the AppImage run here (this machine's glibc must be at
# least the build's); the .deb installs with apt in ubuntu:24.04 and 26.04 and
# the .rpm with dnf in fedora:43 and 44, their dependencies taken from the
# distribution, so a
# wrong package name fails here. Needs Docker for those two; a package that is
# absent is skipped and said so. Exits non-zero on the first failure.
set -euo pipefail
cd "$(dirname "$0")/.."

[ $# -eq 1 ] || { echo "usage: $0 <package dir>" >&2; exit 2; }
dir=$(realpath "$1")
die() { echo "FAIL: $*" >&2; exit 1; }

metadata=$(cargo metadata --no-deps --format-version 1 --locked)
version=$(python3 -I -c '
import json, sys
print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "twicecat-app"))
' <<<"$metadata")
want="twicecat $version"
app_id=io.github.ga_lep.Twicecat

# The checks inside an installed prefix: run as a script with PREFIX and WANT.
read -r -d '' check <<'CHECK' || true
set -eu
bin="$PREFIX/bin/twicecat"
got=$("$bin" --version)
[ "$got" = "$WANT" ] || { echo "FAIL: --version printed '$got', expected '$WANT'"; exit 1; }
if ldd "$bin" | grep -q 'not found'; then ldd "$bin"; echo "FAIL: unresolved libraries"; exit 1; fi
for f in lib/twicecat/models/deepfilternet3/enc.onnx \
         lib/twicecat/models/deepfilternet3-stateful/deepfilternet3_stateful.onnx \
         share/doc/twicecat/LICENSE share/doc/twicecat/THIRD_PARTY_NOTICES.html \
         share/doc/twicecat/OFL.txt share/doc/twicecat/THIRD-PARTY-NOTICES.md \
         share/applications/APP_ID.desktop share/metainfo/APP_ID.metainfo.xml \
         share/icons/hicolor/256x256/apps/APP_ID.png; do
    [ -f "$PREFIX/$f" ] || { echo "FAIL: missing $PREFIX/$f"; exit 1; }
done
ls "$PREFIX"/lib/twicecat/onnxruntime/1.28.3/*/lib/libonnxruntime.so >/dev/null || { echo "FAIL: no bundled ONNX Runtime"; exit 1; }
ls "$PREFIX"/lib/twicecat/onnxruntime/webgpu-ep-*/libonnxruntime_providers_webgpu.so >/dev/null || { echo "FAIL: no bundled WebGPU plugin"; exit 1; }
# share/twicecat is the app's data directory when the prefix is ~/.local.
[ ! -e "$PREFIX/share/twicecat" ] || { echo "FAIL: the package installs into $PREFIX/share/twicecat, the data directory under ~/.local"; exit 1; }
echo "ok: $got, libraries resolved, resources in $PREFIX/lib and $PREFIX/share"
CHECK
check=${check//APP_ID/$app_id}

ran=0
shopt -s nullglob
for tarball in "$dir"/twicecat-"$version"-linux-*.tar.zst; do
    [[ "$tarball" == *-symbols.tar.zst ]] && continue
    echo "== $(basename "$tarball")" >&2
    work=$(mktemp -d)
    tar -C "$work" -I zstd -xf "$tarball"
    PREFIX=$(echo "$work"/twicecat-*) WANT=$want sh -c "$check" || die "$(basename "$tarball")"
    rm -rf "$work"
    ran=$((ran + 1))
done
for appimage in "$dir"/Twicecat-"$version"-*.AppImage; do
    echo "== $(basename "$appimage")" >&2
    got=$("$appimage" --appimage-extract-and-run --version)
    [ "$got" = "$want" ] || die "$(basename "$appimage") printed '$got'"
    work=$(mktemp -d)
    (cd "$work" && "$appimage" --appimage-extract >/dev/null)
    PREFIX=$work/squashfs-root/usr WANT=$want sh -c "$check" || die "$(basename "$appimage")"
    rm -rf "$work"
    ran=$((ran + 1))
done
# The oldest release each package serves (glibc 2.39 and 2.42, plan Q14) and
# the newest, so a dependency name that changed between them fails here.
for deb in "$dir"/twicecat_"$version"*.deb; do
    for image in ubuntu:24.04 ubuntu:26.04; do
        echo "== $(basename "$deb") in $image" >&2
        # The image is minimized: its dpkg drops /usr/share/doc, where the
        # licence and notices go. A desktop installs them, so the test does too.
        docker run --rm -v "$deb:/tmp/twicecat.deb:ro" -e PREFIX=/usr -e WANT="$want" "$image" sh -c \
            "rm -f /etc/dpkg/dpkg.cfg.d/excludes && apt-get update -qq >/dev/null && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq /tmp/twicecat.deb >/dev/null || exit 1; $check" \
            || die "$(basename "$deb") in $image"
        ran=$((ran + 1))
    done
done
for rpm in "$dir"/twicecat-"$version"*.rpm; do
    for image in fedora:43 fedora:44; do
        echo "== $(basename "$rpm") in $image" >&2
        docker run --rm -v "$rpm:/tmp/twicecat.rpm:ro" -e PREFIX=/usr -e WANT="$want" "$image" sh -c \
            "dnf install -y -q /tmp/twicecat.rpm >/dev/null || exit 1; $check" \
            || die "$(basename "$rpm") in $image"
        ran=$((ran + 1))
    done
done
[ "$ran" -gt 0 ] || die "no packages for $version in $dir"
echo "all $ran packages passed" >&2
