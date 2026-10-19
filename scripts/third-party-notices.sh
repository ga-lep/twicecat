#!/usr/bin/env bash
# Writes THIRD_PARTY_NOTICES.html for one build target: the licences of every
# crate the app ships on that target (cargo-about, offline, from about.toml's
# accepted list), then what is not a crate: LiveKit's libwebrtc (linked into
# the binary), the DeepFilterNet3 models (bundled everywhere), the Inter
# typeface (embedded in the binary; packages also ship its OFL.txt), and,
# when their directories are given, ONNX Runtime and its WebGPU plugin
# (bundled in the Linux packages, Phase 7 plan Q3). Every package ships the
# page beside LICENSE (GPL-3.0-or-later).
#
#   scripts/third-party-notices.sh <output.html> [--target <triple>]
#       [--onnxruntime <dir>] [--webgpu-ep <dir>]
#
# --target defaults to the host. libwebrtc's licence comes from
# $LK_CUSTOM_WEBRTC (else dev/libwebrtc), the fetched archive for this target.
# Needs cargo-about 0.9.2 (`cargo install cargo-about@0.9.2 --locked
# --features cli`) and the crates already fetched (`cargo fetch --locked`):
# it runs with --frozen, so nothing is downloaded. It fails when a crate's
# licence is not in about.toml's list, or a notice file is missing.
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
    echo "usage: $0 <output.html> [--target <triple>] [--onnxruntime <dir>] [--webgpu-ep <dir>]" >&2
    exit 2
}
[ $# -ge 1 ] || usage
out=$1
shift
target=$(rustc -vV | awk '/^host:/ { print $2 }')
onnxruntime=""
webgpu_ep=""
while [ $# -gt 0 ]; do
    case "$1" in
        --target) [ $# -ge 2 ] || usage; target=$2; shift 2 ;;
        --onnxruntime) [ $# -ge 2 ] || usage; onnxruntime=$2; shift 2 ;;
        --webgpu-ep) [ $# -ge 2 ] || usage; webgpu_ep=$2; shift 2 ;;
        *) usage ;;
    esac
done
command -v cargo-about >/dev/null || {
    echo "error: cargo-about is not installed (cargo install cargo-about@0.9.2 --locked --features cli)" >&2
    exit 1
}

# need FILE: fails unless FILE is a readable regular file.
need() {
    if [ ! -f "$1" ] || [ ! -r "$1" ]; then
        echo "error: missing notice file $1" >&2
        exit 1
    fi
}

webrtc=${LK_CUSTOM_WEBRTC:-dev/libwebrtc}
need "$webrtc/LICENSE.md"
need packaging/licenses/DeepFilterNet-LICENSE-MIT
need packaging/licenses/DeepFilterNet-LICENSE-APACHE
need crates/app/assets/fonts/inter/OFL.txt
extra=(
    "libwebrtc (LiveKit's build of WebRTC, linked into Twicecat)" "$webrtc/LICENSE.md"
    "DeepFilterNet3 models (Rikorose/DeepFilterNet v0.5.6), MIT licence" packaging/licenses/DeepFilterNet-LICENSE-MIT
    "DeepFilterNet3 models (Rikorose/DeepFilterNet v0.5.6), Apache licence" packaging/licenses/DeepFilterNet-LICENSE-APACHE
    "Inter 4.1 typeface (embedded in Twicecat), SIL Open Font License 1.1" crates/app/assets/fonts/inter/OFL.txt
)
if [ -n "$onnxruntime" ]; then
    need "$onnxruntime/LICENSE"
    need "$onnxruntime/ThirdPartyNotices.txt"
    extra+=(
        "ONNX Runtime (Microsoft), licence" "$onnxruntime/LICENSE"
        "ONNX Runtime (Microsoft), third-party notices" "$onnxruntime/ThirdPartyNotices.txt"
    )
fi
if [ -n "$webgpu_ep" ]; then
    need "$webgpu_ep/licenses/LICENSE"
    need "$webgpu_ep/licenses/ThirdPartyNotices.txt"
    extra+=(
        "ONNX Runtime WebGPU plugin (Microsoft), licence" "$webgpu_ep/licenses/LICENSE"
        "ONNX Runtime WebGPU plugin (Microsoft), third-party notices" "$webgpu_ep/licenses/ThirdPartyNotices.txt"
    )
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# The app's dependency graph on the target, with the GPU feature every
# package is built with (plan Q3).
cargo about generate --format json --frozen --fail -c about.toml \
    -m crates/app/Cargo.toml --features gpu --target "$target" -o "$work/crates.json"
# Capture first so a cargo failure aborts the script (set -e).
metadata=$(cargo metadata --no-deps --format-version 1 --locked)
version=$(python3 -I -c '
import json, sys
print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "twicecat-app"))
' <<<"$metadata")

python3 -I - "$work/crates.json" "$out" "$target" "$version" "${extra[@]}" <<'EOF'
import html, json, sys

crates_json, out, target, version, *extra = sys.argv[1:]
about = json.load(open(crates_json, encoding="utf-8"))
e = html.escape

parts = [f"""<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<title>Twicecat {e(version)}: third-party notices</title>
<style>
body {{ font-family: sans-serif; max-width: 52rem; margin: 0 auto; padding: 1rem; }}
pre {{ white-space: pre-wrap; max-height: 20rem; overflow-y: auto; background: #f4f4f4; padding: .5rem; }}
@media (prefers-color-scheme: dark) {{ body {{ background: #1e1e1e; color: #ddd; }} pre {{ background: #2a2a2a; }} a {{ color: #8ab4ff; }} }}
</style></head><body>
<h1>Twicecat {e(version)}: third-party notices</h1>
<p>Twicecat is free software under the GNU General Public License, version 3 or later
(the LICENSE file beside this one). Its source code for this version is at
<a href="https://github.com/ga-lep/twicecat/tree/v{e(version)}">github.com/ga-lep/twicecat/tree/v{e(version)}</a>.
This build ({e(target)}) includes the software below, under the licences shown.</p>
<h2>Rust crates</h2>
<ul>"""]
for item in about["overview"]:
    parts.append(f'<li><a href="#{e(item["id"])}">{e(item["name"])}</a> ({item["count"]})</li>')
parts.append("</ul>")
for i, licence in enumerate(about["licenses"]):
    if licence["first_of_kind"]:
        parts.append(f'<h3 id="{e(licence["id"])}">{e(licence["name"])}</h3>')
    users = ", ".join(f'{e(u["crate"]["name"])} {e(u["crate"]["version"])}' for u in licence["used_by"])
    parts.append(f"<p>Used by: {users}</p><pre>{e(licence['text'])}</pre>")
parts.append("<h2>Other components</h2>")
for title, path in zip(extra[0::2], extra[1::2]):
    text = open(path, encoding="utf-8", errors="replace").read()
    parts.append(f"<h3>{e(title)}</h3><pre>{e(text)}</pre>")
parts.append("</body></html>\n")
with open(out, "w", encoding="utf-8") as f:
    f.write("\n".join(parts))
print(f"wrote {out}: {len(about['crates'])} crates, {len(extra) // 2} other notices", file=sys.stderr)
EOF
