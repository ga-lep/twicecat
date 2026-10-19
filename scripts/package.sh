#!/usr/bin/env bash
# Builds Twicecat and packages it for this machine (Phase 7 plan Task 6).
#
#   scripts/package.sh [--profile dist|release] [--formats "tar deb rpm appimage"]
#                      [--no-build]
#   scripts/package.sh --formats prefix --prefix DIR --notices FILE
#
# Formats: on Linux `tar` (.tar.zst), `deb` and `rpm` (nfpm), `appimage`
# (appimagetool, from our own AppDir), all by default; on macOS `dmg`; on
# Windows `msi` (both cargo-packager). `--formats packager-check` on Linux
# runs cargo-packager's .deb on the same generated config as a dry run of the
# DMG and MSI configuration (its output is thrown away). `--formats prefix`
# (Linux, alone) installs the same tree straight into DIR, with no symbols
# archive and the notices page copied from FILE: the Flatpak build
# (packaging/flatpak/) installs into /app this way, offline, where
# cargo-about cannot run.
#
# Every package carries the full build (`--features gpu`, plan Q3), the
# DeepFilterNet3 models, LICENSE, THIRD_PARTY_NOTICES.html and the Inter
# font's OFL.txt with THIRD-PARTY-NOTICES.md; Linux packages and the Windows
# MSI also the "any GPU" pack: ONNX Runtime's CPU build, its WebGPU plugin and
# the stateful graph (plan Q3 b; the MSI since 2026-10-18, human request), in
# the layout crates/audio/src/paths.rs reads (beside the .exe on Windows;
# <prefix>/lib/twicecat on Linux: not share/, which is the data directory when
# the .tar.zst is unpacked into ~/.local). Linux packages ship a stripped binary; the
# unstripped one goes into a separate symbols archive (plan Q4).
#
# Inputs come from the pinned fetch scripts (models, ONNX Runtime, the WebGPU
# plugin, the packaging tools); the stateful graph from
# scripts/export-deepfilternet.sh's cache or TWICECAT_STATEFUL_MODEL, checked
# against its pinned SHA-256. Output: <target dir>/package/<triple>/. Linux packages
# need the build machine's glibc or newer: release CI builds in dev/ubuntu-24.04
# (2.39, plan Q14), and the script stops if the binary needs more than the
# floor packaging/nfpm.yaml declares.
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
    echo "usage: $0 [--profile dist|release] [--formats \"tar deb rpm appimage\"] [--no-build]" >&2
    echo "       $0 --formats prefix --prefix DIR --notices FILE [--profile dist|release] [--no-build]" >&2
    exit 2
}
die() { echo "error: $*" >&2; exit 1; }

sha256_of() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

profile=dist
formats=""
build=1
prefix=""
notices=""
while [ $# -gt 0 ]; do
    case "$1" in
        --profile) [ $# -ge 2 ] || usage; profile=$2; shift 2 ;;
        --formats) [ $# -ge 2 ] || usage; formats=$2; shift 2 ;;
        --no-build) build=0; shift ;;
        --prefix) [ $# -ge 2 ] || usage; prefix=$2; shift 2 ;;
        --notices) [ $# -ge 2 ] || usage; notices=$2; shift 2 ;;
        *) usage ;;
    esac
done
case "$profile" in dist | release) ;; *) usage ;; esac
if [ "$formats" = prefix ]; then
    if [ -z "$prefix" ] || [ -z "$notices" ]; then usage; fi
    [ -f "$notices" ] || die "no notices page at $notices"
elif [ -n "$prefix" ] || [ -n "$notices" ]; then
    usage
fi

app_id=io.github.ga_lep.Twicecat
host=$(rustc -vV | awk '/^host:/ { print $2 }')
arch=${host%%-*}
case "$host" in
    *-linux-*) os=linux ;;
    *-apple-darwin) os=macos ;;
    *-windows-*) os=windows ;;
    *) die "no packaging for $host" ;;
esac
if [ -z "$formats" ]; then
    case "$os" in
        linux) formats="tar deb rpm appimage" ;;
        macos) formats="dmg" ;;
        windows) formats="msi" ;;
    esac
fi
for f in $formats; do
    case "$os:$f" in
        linux:tar | linux:deb | linux:rpm | linux:appimage | linux:packager-check | linux:prefix | macos:dmg | windows:msi) ;;
        *) die "format $f is not made on $os" ;;
    esac
done

# Capture first so a cargo failure aborts the script (set -e).
metadata=$(cargo metadata --no-deps --format-version 1 --locked)
version=$(python3 -I -c '
import json, sys
print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "twicecat-app"))
' <<<"$metadata")

target_dir=${CARGO_TARGET_DIR:-target}
out=$target_dir/package/$host
exe=twicecat
[ "$os" = windows ] && exe=twicecat.exe
binary=$target_dir/$host/$profile/$exe
if [ "$build" -eq 1 ]; then
    echo "building twicecat $version ($host, profile $profile, --features gpu)..." >&2
    cargo build --locked --profile "$profile" --target "$host" -p twicecat-app --bin twicecat --features gpu
fi
[ -f "$binary" ] || die "$binary not built (drop --no-build)"
rm -rf "$out"
mkdir -p "$out"

# --- What every package carries -------------------------------------------

models=$(scripts/fetch-models.sh)
# The notices read the dependency graph offline (--frozen), which needs the
# crates of every platform in the registry, not only those this build used.
[ -n "$notices" ] || cargo fetch --locked
notices_args=()
gpu_pack=0
case "$os" in linux | windows) gpu_pack=1 ;; esac

# font_notices DIR: the Inter typeface's licence and the notice that names
# it, which Settings -> About points to (the OFL asks for both beside the
# embedded font).
font_notices() {
    cp crates/app/assets/fonts/inter/OFL.txt "$1/OFL.txt"
    cp THIRD-PARTY-NOTICES.md "$1/THIRD-PARTY-NOTICES.md"
}

# resources DIR: the resource tree (the layout of the user cache) under DIR.
resources() {
    local dir=$1
    mkdir -p "$dir/models"
    cp -R "$models" "$dir/models/deepfilternet3"
    rm -f "$dir/models/deepfilternet3/.verified"
    if [ "$gpu_pack" -eq 1 ]; then
        local stateful expected actual ort ep platform
        stateful=${TWICECAT_STATEFUL_MODEL:-${XDG_CACHE_HOME:-$HOME/.cache}/twicecat/models/deepfilternet3-stateful/deepfilternet3_stateful.onnx}
        [ -f "$stateful" ] || die "no stateful graph at $stateful: run scripts/export-deepfilternet.sh or set TWICECAT_STATEFUL_MODEL"
        expected=$(awk '$2 == "deepfilternet3_stateful.onnx" { print $1 }' scripts/checksums/models.sha256)
        actual=$(sha256_of "$stateful")
        [ "$actual" = "$expected" ] || die "the stateful graph's SHA-256 is $actual, expected $expected"
        mkdir -p "$dir/models/deepfilternet3-stateful"
        cp "$stateful" "$dir/models/deepfilternet3-stateful/"
        cp "$models/config.ini" "$dir/models/deepfilternet3-stateful/"

        ort=$(scripts/fetch-onnxruntime.sh cpu)
        ep=$(scripts/fetch-webgpu-ep.sh)
        platform=$(basename "$(dirname "$ort")")
        mkdir -p "$dir/onnxruntime/1.28.3/$platform/lib" "$dir/onnxruntime/$(basename "$ep")"
        # The libraries only (symbolic links kept), not headers, import
        # libraries, symbols or CMake files. On Windows the plugin comes with
        # the DirectX shader compiler Direct3D 12 needs (fetch-webgpu-ep.sh).
        if [ "$os" = windows ]; then
            cp "$ort"/*.dll "$dir/onnxruntime/1.28.3/$platform/lib/"
            cp "$ep"/*.dll "$dir/onnxruntime/$(basename "$ep")/"
        else
            cp -P "$ort"/libonnxruntime*.so* "$dir/onnxruntime/1.28.3/$platform/lib/"
            cp -P "$ep"/*.so "$dir/onnxruntime/$(basename "$ep")/"
        fi
        notices_args=(--onnxruntime "$(dirname "$ort")" --webgpu-ep "$ep")
    fi
}

# --- Linux ----------------------------------------------------------------

if [ "$os" = linux ]; then
    case "$arch" in
        x86_64) loader=ld-linux-x86-64.so.2 ;;
        aarch64) loader=ld-linux-aarch64.so.1 ;;
        *) die "no packaging for $arch" ;;
    esac
    if [ "$formats" != prefix ]; then
        # The libraries the binary links must be the ones nfpm.yaml depends on.
        needed=$(readelf -d "$binary" | awk '/\(NEEDED\)/ { gsub(/[\[\]]/, "", $5); print $5 }' | sort | tr '\n' ' ')
        wanted=$(printf '%s\n' libasound.so.2 libc.so.6 libgcc_s.so.1 libm.so.6 libpipewire-0.3.so.0 libudev.so.1 "$loader" | sort | tr '\n' ' ')
        [ "$needed" = "$wanted" ] || die "the binary links [$needed], packaging/nfpm.yaml expects [$wanted]: update both"
        # Nor may it need a newer glibc than the floor nfpm.yaml declares, so
        # a newer build image cannot raise the floor silently (plan Q14).
        floor=$(tr -d '\r' < packaging/nfpm.yaml | sed -n 's/^ *- libc6 (>= \([0-9.]*\))$/\1/p')
        rpm_floor=$(tr -d '\r' < packaging/nfpm.yaml | sed -n 's/^ *- glibc >= \([0-9.]*\)$/\1/p')
        { [ -n "$floor" ] && [ "$floor" = "$rpm_floor" ]; } \
            || die "packaging/nfpm.yaml: the libc6 floor ($floor) and the glibc floor ($rpm_floor) must be set and equal"
        newest=$(objdump -T "$binary" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -uV | tail -n 1)
        [ -n "$newest" ] || die "no glibc symbol versions in $binary"
        [ "$(printf '%s\n%s\n' "$newest" "$floor" | sort -V | tail -n 1)" = "$floor" ] \
            || die "the binary needs glibc $newest, packaging/nfpm.yaml declares $floor: build in dev/ubuntu-24.04"
        echo "glibc: the binary needs $newest, the packages declare $floor" >&2
    fi

    stage=$out/stage
    usr=$stage/usr
    [ "$formats" = prefix ] && usr=$prefix
    mkdir -p "$usr/bin" "$usr/lib/twicecat" "$usr/share/applications" "$usr/share/metainfo" \
        "$usr/share/icons" "$usr/share/doc/twicecat"
    # Stripped in the packages; the unstripped binary is the symbols archive.
    strip -o "$usr/bin/twicecat" "$binary"
    chmod 755 "$usr/bin/twicecat"
    resources "$usr/lib/twicecat"
    cp "packaging/linux/$app_id.desktop" "$usr/share/applications/"
    cp "packaging/linux/$app_id.metainfo.xml" "$usr/share/metainfo/"
    cp -R packaging/icons/hicolor "$usr/share/icons/"
    cp LICENSE "$usr/share/doc/twicecat/LICENSE"
    font_notices "$usr/share/doc/twicecat"
    if [ -n "$notices" ]; then
        cp "$notices" "$usr/share/doc/twicecat/THIRD_PARTY_NOTICES.html"
    else
        scripts/third-party-notices.sh "$usr/share/doc/twicecat/THIRD_PARTY_NOTICES.html" --target "$host" "${notices_args[@]}"
    fi
    if [ "$formats" = prefix ]; then
        echo "installed twicecat $version into $prefix" >&2
        exit 0
    fi

    name=twicecat-$version-linux-$arch
    symbols=$out/$name-symbols
    mkdir -p "$symbols"
    cp "$binary" "$symbols/twicecat"
    tar -C "$out" -I 'zstd -19 -T0' -cf "$out/$name-symbols.tar.zst" "$name-symbols"
    rm -rf "$symbols"

    tools=""
    for f in $formats; do
        case "$f" in deb | rpm | appimage) tools=$(scripts/fetch-packaging-tools.sh) ;; esac
    done
    for f in $formats; do
        case "$f" in
            tar)
                # <name>/bin, <name>/lib and <name>/share: the prefix layout, anywhere.
                mkdir -p "$out/$name"
                cp -a "$usr/." "$out/$name/"
                tar -C "$out" -I 'zstd -19 -T0' -cf "$out/$name.tar.zst" "$name"
                rm -rf "${out:?}/$name"
                ;;
            deb | rpm)
                deb_arch=$arch
                case "$arch" in x86_64) deb_arch=amd64 ;; aarch64) deb_arch=arm64 ;; esac
                TWICECAT_VERSION=$version TWICECAT_ARCH=$deb_arch TWICECAT_STAGE=$(realpath "$stage") \
                    "$tools/nfpm" package -f packaging/nfpm.yaml -p "$f" -t "$out/"
                ;;
            appimage)
                appdir=$out/AppDir
                mkdir -p "$appdir"
                cp -a "$usr" "$appdir/usr"
                ln -s usr/bin/twicecat "$appdir/AppRun"
                cp "packaging/linux/$app_id.desktop" "$appdir/"
                cp "packaging/icons/hicolor/256x256/apps/$app_id.png" "$appdir/"
                ln -s "$app_id.png" "$appdir/.DirIcon"
                # appimagetool is itself an AppImage: extract-and-run needs no FUSE.
                ARCH=$arch "$tools/appimagetool" --appimage-extract-and-run --no-appstream \
                    --runtime-file "$tools/runtime" "$appdir" "$out/Twicecat-$version-$arch.AppImage" >&2
                rm -rf "$appdir"
                ;;
        esac
    done
fi

# --- cargo-packager (macOS DMG, Windows MSI; a .deb dry run on Linux) -----

packager_config() {
    local res=$1 config=$2
    # macOS's realpath refuses a path that does not exist yet.
    mkdir -p "$out/packager"
    python3 -I - "$config" "$version" "$(realpath "$(dirname "$binary")")" "$(realpath "$res")" \
        "$(realpath "$out/packager")" "$app_id" <<'EOF'
import json, os, sys
config_path, version, binaries_dir, res, out_dir, app_id = sys.argv[1:]
# cargo-packager reads relative paths from the config's directory, not ours.
repo = os.getcwd()
icons = os.path.join(repo, "packaging/icons/hicolor")
macos = os.path.join(repo, "packaging/macos")
config = {
    "productName": "Twicecat",
    "name": "twicecat",
    "version": version,
    "identifier": app_id,
    "description": "Encrypted chat and voice calls on Matrix",
    "homepage": "https://github.com/ga-lep/twicecat",
    "authors": ["The Twicecat developers"],
    "publisher": "The Twicecat developers",
    "licenseFile": os.path.join(repo, "LICENSE"),
    "category": "SocialNetworking",
    "outDir": out_dir,
    "binariesDir": binaries_dir,
    "binaries": [{"path": "twicecat", "main": True}],
    # The resource tree goes where crates/audio/src/paths.rs looks: the
    # bundle's Contents/Resources on macOS, beside the .exe on Windows.
    "resources": [{"src": f"{res}/models", "target": "models"},
                  {"src": f"{res}/LICENSE", "target": "LICENSE"},
                  {"src": f"{res}/THIRD_PARTY_NOTICES.html", "target": "THIRD_PARTY_NOTICES.html"},
                  {"src": f"{res}/OFL.txt", "target": "OFL.txt"},
                  {"src": f"{res}/THIRD-PARTY-NOTICES.md", "target": "THIRD-PARTY-NOTICES.md"}]
                 # The "any GPU" pack, when the package carries it (the MSI).
                 + ([{"src": f"{res}/onnxruntime", "target": "onnxruntime"}]
                    if os.path.isdir(f"{res}/onnxruntime") else []),
    "icons": [f"{icons}/{n}x{n}/apps/{app_id}.png" for n in (16, 32, 64, 128, 256, 512)]
             + [os.path.join(repo, "packaging/icons", app_id + ".ico")],
    "macos": {
        "minimumSystemVersion": "13.0",
        "infoPlistPath": os.path.join(macos, "Info.plist"),
        "entitlements": os.path.join(macos, "entitlements.plist"),
    },
    "wix": {"languages": ["en-US"]},
}
# Signing (plan Q6), only when the release workflow has the human's secrets.
# macOS: cargo-packager signs the .app with the hardened runtime and the
# entitlements, notarizes and staples it when APPLE_API_KEY, APPLE_API_ISSUER
# and APPLE_API_KEY_PATH are set, then signs the DMG (APPLE_CERTIFICATE and
# APPLE_CERTIFICATE_PASSWORD go into a temporary keychain). Windows: a sign
# command with %1 for the file, run on twicecat.exe and on the MSI.
identity = os.environ.get("APPLE_SIGNING_IDENTITY")
if identity:
    config["macos"]["signingIdentity"] = identity
sign_command = os.environ.get("TWICECAT_WINDOWS_SIGN_COMMAND")
if sign_command:
    if "%1" not in sign_command:
        sys.exit("TWICECAT_WINDOWS_SIGN_COMMAND needs %1 for the file to sign")
    config["windows"] = {"signCommand": sign_command}
with open(config_path, "w", encoding="utf-8") as f:
    json.dump(config, f, indent=2)
EOF
}

packager_formats=""
for f in $formats; do
    # cargo-packager names its MSI format after the toolset that builds it.
    case "$f" in dmg) packager_formats="$packager_formats dmg" ;; msi) packager_formats="$packager_formats wix" ;; packager-check) packager_formats=" deb" ;; esac
done
if [ -n "$packager_formats" ]; then
    command -v cargo-packager >/dev/null || die "cargo-packager is not installed (cargo install cargo-packager@0.11.8 --locked)"
    res=$out/resources
    # The MSI carries the pack; the DMG does not, nor the Linux dry run.
    if [ "$os" != windows ]; then gpu_pack=0; fi
    notices_args=()
    resources "$res"
    cp LICENSE "$res/LICENSE"
    font_notices "$res"
    scripts/third-party-notices.sh "$res/THIRD_PARTY_NOTICES.html" --target "$host" "${notices_args[@]}"
    packager_config "$res" "$out/packager.json"
    for f in $packager_formats; do
        cargo packager --config "$out/packager.json" --formats "$f"
    done
    if [ "$formats" = packager-check ]; then
        rm -rf "$out/packager"
    fi
fi

echo "packages in $out:" >&2
find "$out" -maxdepth 3 -type f \( -name '*.tar.zst' -o -name '*.deb' -o -name '*.rpm' -o -name '*.AppImage' -o -name '*.dmg' -o -name '*.msi' \) -exec ls -l {} + >&2
