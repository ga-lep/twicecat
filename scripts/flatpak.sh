#!/usr/bin/env bash
# Twicecat's Flatpak (Phase 7 plan Task 7): packaging/flatpak/.
#
#   scripts/flatpak.sh sources   regenerates packaging/flatpak/cargo-sources.json
#                                from Cargo.lock
#   scripts/flatpak.sh check     fails unless cargo-sources.json matches
#                                Cargo.lock and every download in the manifest
#                                has the SHA-256 scripts/checksums/ pins
#   scripts/flatpak.sh build     builds the Flatpak for this machine's
#                                architecture and the bundle
#                                <target dir>/package/flatpak/Twicecat-<v>-<arch>.flatpak
#   scripts/flatpak.sh smoke     installs that bundle for this user and checks
#                                `twicecat --version` and the bundled files
#
# `sources` and `check` run flatpak-cargo-generator.py from
# flatpak/flatpak-builder-tools at a pinned commit (its SHA-256 below), in a
# venv with its two dependencies pinned (tomlkit, aiohttp). `build` needs
# flatpak and flatpak-builder (from the distribution), the stateful graph
# (scripts/export-deepfilternet.sh, or TWICECAT_STATEFUL_MODEL) and the
# notices page: TWICECAT_FLATPAK_NOTICES (the .deb's page, as release CI
# passes it), or else cargo-about 0.9.2 makes it here, since the sandbox has
# no network. The runtime and SDK come from Flathub, installed for this user.
set -euo pipefail
cd "$(dirname "$0")/.."

app_id=io.github.ga_lep.Twicecat
manifest=packaging/flatpak/$app_id.yml
sources_json=packaging/flatpak/cargo-sources.json
generator_commit=41c20aa10819cdb2a4f3ca171758a96d1955c018
generator_sha256=0a2db6be87d75910facef28ab46d4d6460802e8419ab850d0caa6a364d26b380
python_deps=(tomlkit==0.15.1 aiohttp==3.14.3)
flathub=https://dl.flathub.org/repo/flathub.flatpakrepo

usage() {
    echo "usage: $0 sources | check | build | smoke" >&2
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

[ $# -eq 1 ] || usage
cache_root="${TWICECAT_TOOLS_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/twicecat/tools}/flatpak"
target_dir=${CARGO_TARGET_DIR:-target}

# generate OUT: cargo-sources.json from Cargo.lock into OUT.
generate() {
    local out=$1 script venv python
    mkdir -p "$cache_root"
    script=$cache_root/flatpak-cargo-generator-$generator_commit.py
    if [ ! -f "$script" ] || [ "$(sha256_of "$script")" != "$generator_sha256" ]; then
        local url="https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/$generator_commit/cargo/flatpak-cargo-generator.py"
        echo "downloading $url" >&2
        curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$script.part" "$url"
        [ "$(sha256_of "$script.part")" = "$generator_sha256" ] || die "SHA-256 mismatch for flatpak-cargo-generator.py"
        mv "$script.part" "$script"
    fi
    venv=$cache_root/venv-$(printf '%s ' "${python_deps[@]}" | sha256sum | cut -c1-12)
    if [ ! -d "$venv" ]; then
        python3 -m venv "$venv.part"
        if [ -x "$venv.part/bin/python" ]; then python=$venv.part/bin/python; else python=$venv.part/Scripts/python.exe; fi
        "$python" -m pip install --quiet --disable-pip-version-check "${python_deps[@]}"
        mv "$venv.part" "$venv"
    fi
    if [ -x "$venv/bin/python" ]; then python=$venv/bin/python; else python=$venv/Scripts/python.exe; fi
    # The lock has no git dependencies, so the generator only reads it (no
    # network).
    "$python" -I "$script" Cargo.lock -o "$out" 2>/dev/null
}

# check_pins: every url/sha256 pair in the manifest against scripts/checksums/.
check_pins() {
    python3 -I - "$manifest" scripts/checksums/*.sha256 <<'EOF'
import re, sys

manifest, *sums = sys.argv[1:]
pinned = {}
for path in sums:
    for line in open(path, encoding="utf-8"):
        fields = line.split()
        if not fields or fields[0].startswith("#"):
            continue
        for field in fields[1:]:
            pinned.setdefault(field.rsplit("/", 1)[-1], set()).add(fields[0])
text = open(manifest, encoding="utf-8").read()
pairs = re.findall(r"url: (\S+)\s*\n\s*sha256: ([0-9a-f]{64})", text)
urls = re.findall(r"url: (\S+)", text)
bad = []
if len(pairs) != len(urls):
    bad.append(f"{len(urls) - len(pairs)} url(s) without a sha256 on the next line")
for url, sha in pairs:
    name = url.rsplit("/", 1)[-1]
    if name not in pinned:
        bad.append(f"{name}: not pinned in scripts/checksums/")
    elif sha not in pinned[name]:
        bad.append(f"{name}: manifest has {sha}, scripts/checksums/ has {sorted(pinned[name])}")
for line in bad:
    print(f"error: {manifest}: {line}", file=sys.stderr)
if bad:
    sys.exit(1)
print(f"ok: {len(pairs)} downloads in {manifest} match scripts/checksums/", file=sys.stderr)
EOF
}

# The workspace version, read without cargo (flatpak-builder's CI container
# has none); scripts/check-version.sh checks that every crate inherits it.
version() {
    local v
    v=$(awk '/^\[/ { section = $0 } section == "[workspace.package]" && $1 == "version" { gsub(/"/, "", $3); print $3 }' Cargo.toml | tr -d '\r')
    [ -n "$v" ] || die "no version in Cargo.toml's [workspace.package]"
    printf '%s\n' "$v"
}

arch=$(uname -m)
case "$arch" in arm64) arch=aarch64 ;; esac
bundle_dir=$target_dir/package/flatpak

case "$1" in
    sources)
        generate "$sources_json"
        echo "wrote $sources_json" >&2
        ;;
    check)
        work=$(mktemp -d)
        trap 'rm -rf "$work"' EXIT
        generate "$work/cargo-sources.json"
        diff -q --strip-trailing-cr "$work/cargo-sources.json" "$sources_json" >/dev/null ||
            die "$sources_json does not match Cargo.lock: run scripts/flatpak.sh sources"
        echo "ok: $sources_json matches Cargo.lock" >&2
        check_pins
        ;;
    build)
        case "$arch" in x86_64 | aarch64) ;; *) die "no Flatpak for $arch" ;; esac
        command -v flatpak-builder >/dev/null || die "flatpak-builder is not installed"
        v=$(version)
        inputs=$target_dir/flatpak/inputs
        mkdir -p "$inputs" "$bundle_dir"
        stateful=${TWICECAT_STATEFUL_MODEL:-${XDG_CACHE_HOME:-$HOME/.cache}/twicecat/models/deepfilternet3-stateful/deepfilternet3_stateful.onnx}
        [ -f "$stateful" ] || die "no stateful graph at $stateful: run scripts/export-deepfilternet.sh or set TWICECAT_STATEFUL_MODEL"
        expected=$(awk '$2 == "deepfilternet3_stateful.onnx" { print $1 }' scripts/checksums/models.sha256)
        [ "$(sha256_of "$stateful")" = "$expected" ] || die "the stateful graph's SHA-256 is not $expected"
        cp "$stateful" "$inputs/deepfilternet3_stateful.onnx"
        # The notices for this target with the "any GPU" pack the Flatpak
        # bundles: given, or made from the same pinned archives.
        if [ -n "${TWICECAT_FLATPAK_NOTICES:-}" ]; then
            cp "$TWICECAT_FLATPAK_NOTICES" "$inputs/THIRD_PARTY_NOTICES-$arch.html"
        else
            ort=$(scripts/fetch-onnxruntime.sh cpu)
            ep=$(scripts/fetch-webgpu-ep.sh)
            cargo fetch --locked
            LK_CUSTOM_WEBRTC=$(scripts/fetch-libwebrtc.sh) scripts/third-party-notices.sh \
                "$inputs/THIRD_PARTY_NOTICES-$arch.html" --target "$arch-unknown-linux-gnu" \
                --onnxruntime "$(dirname "$ort")" --webgpu-ep "$ep"
        fi
        flatpak remote-add --user --if-not-exists flathub "$flathub"
        flatpak-builder --user --install-deps-from=flathub --force-clean --disable-rofiles-fuse \
            --state-dir="$target_dir/flatpak/state" --repo="$target_dir/flatpak/repo" \
            "$target_dir/flatpak/build" "$manifest"
        bundle=$bundle_dir/Twicecat-$v-$arch.flatpak
        flatpak build-bundle --runtime-repo="$flathub" "$target_dir/flatpak/repo" "$bundle" "$app_id"
        ls -l "$bundle" >&2
        ;;
    smoke)
        v=$(version)
        bundle=$bundle_dir/Twicecat-$v-$arch.flatpak
        [ -f "$bundle" ] || die "no bundle at $bundle (scripts/flatpak.sh build)"
        flatpak install --user --noninteractive --reinstall "$bundle"
        got=$(flatpak run --command=twicecat "$app_id" --version)
        [ "$got" = "twicecat $v" ] || die "--version printed '$got', expected 'twicecat $v'"
        # Where the app and the desktop look (crates/audio/src/paths.rs:
        # /app/lib/twicecat). The script inside gets the app ID as $1.
        # shellcheck disable=SC2016
        flatpak run --command=sh "$app_id" -c '
            set -e
            for f in lib/twicecat/models/deepfilternet3/enc.onnx \
                     lib/twicecat/models/deepfilternet3-stateful/deepfilternet3_stateful.onnx \
                     share/doc/twicecat/LICENSE share/doc/twicecat/THIRD_PARTY_NOTICES.html \
                     share/doc/twicecat/OFL.txt share/doc/twicecat/THIRD-PARTY-NOTICES.md \
                     "share/applications/$1.desktop" "share/metainfo/$1.metainfo.xml" \
                     "share/icons/hicolor/256x256/apps/$1.png"; do
                [ -f "/app/$f" ] || { echo "missing /app/$f"; exit 1; }
            done
            ls /app/lib/twicecat/onnxruntime/1.28.3/*/lib/libonnxruntime.so >/dev/null
            ls /app/lib/twicecat/onnxruntime/webgpu-ep-*/libonnxruntime_providers_webgpu.so >/dev/null
            [ ! -e /app/share/twicecat ] || { echo "/app/share/twicecat exists: resources go in /app/lib/twicecat"; exit 1; }' sh "$app_id"
        echo "ok: $got, resources in /app/lib and /app/share" >&2
        ;;
    *) usage ;;
esac
