#!/usr/bin/env bash
# Builds the stateful DeepFilterNet3 graph for the GPU speech enhancement
# (plan Task 13b) and installs it next to the fetched models:
#   1. a Python venv with the packages pinned in scripts/export-requirements.txt
#      (PyTorch's CPU build; no GPU needed),
#   2. DeepFilterNet's sources at the pinned commit (tag v0.5.6, the release
#      the fetched models come from) and its DeepFilterNet3 checkpoint,
#      checked against scripts/models.sha256,
#   3. scripts/export-deepfilternet-stateful.py, which exports the graph and
#      compares it frame by frame with upstream's graphs (scripts/fetch-models.sh),
#   4. the result checked against its pinned SHA-256 (the export is
#      reproducible) and installed; its directory is printed.
#
# Usage: scripts/export-deepfilternet.sh
# Env: TESSERA_MODEL_CACHE overrides the model cache root (as fetch-models.sh);
#      TESSERA_EXPORT_CACHE where the venv and sources go (default: the cache
#      root's export/, about 1 GB); PYTHON the interpreter for the venv.
# Progress goes to stderr; stdout carries only the path.
set -euo pipefail
cd "$(dirname "$0")/.."

commit=978576aa8400552a4ce9730838c635aa30db5e61
torch_version=2.14.1
sums=scripts/models.sha256
model=deepfilternet3_stateful.onnx

die() { echo "error: $*" >&2; exit 1; }

sha256_of() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

pinned() {
    local sum
    sum=$(awk -v f="$1" '$2 == f {print $1}' "$sums")
    [[ "$sum" =~ ^[0-9a-f]{64}$ ]] || die "no pinned SHA-256 for $1 in $sums"
    printf '%s' "$sum"
}

expected=$(pinned "$model")
upstream=$(scripts/fetch-models.sh)
cache_root="${TESSERA_MODEL_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/tessera/models}"
dest="$cache_root/deepfilternet3-stateful"
if [ -f "$dest/.verified" ] && [ "$(cat "$dest/.verified")" = "$expected" ]; then
    printf '%s\n' "$dest"
    exit 0
fi

work_root="${TESSERA_EXPORT_CACHE:-$(dirname "$cache_root")/export}"
mkdir -p "$work_root"
venv="$work_root/venv-torch-$torch_version"
if [ ! -x "$venv/bin/python" ]; then
    echo "creating $venv" >&2
    if command -v uv >/dev/null; then
        uv venv -q --python "${PYTHON:-3.13}" "$venv"
        install() { uv pip install -q --python "$venv/bin/python" "$@"; }
    else
        "${PYTHON:-python3}" -m venv "$venv"
        install() { "$venv/bin/pip" install -q "$@"; }
    fi
    install --index-url https://download.pytorch.org/whl/cpu "torch==$torch_version+cpu"
    install -r scripts/export-requirements.txt
fi

src="$work_root/DeepFilterNet-$commit"
if [ ! -d "$src/.git" ] || [ "$(git -C "$src" rev-parse HEAD)" != "$commit" ]; then
    rm -rf "$src"
    echo "fetching DeepFilterNet at $commit" >&2
    git init -q "$src"
    git -C "$src" fetch -q --depth 1 https://github.com/Rikorose/DeepFilterNet.git "$commit"
    git -C "$src" -c advice.detachedHead=false checkout -q FETCH_HEAD
fi
[ "$(git -C "$src" rev-parse HEAD)" = "$commit" ] || die "DeepFilterNet checkout is not at $commit"

zip="$src/models/DeepFilterNet3.zip"
[ "$(sha256_of "$zip")" = "$(pinned DeepFilterNet3.zip)" ] || die "SHA-256 mismatch for DeepFilterNet3.zip"
work=$(mktemp -d "$work_root/.export.XXXXXX")
trap 'rm -rf "$work"' EXIT
"$venv/bin/python" -m zipfile -e "$zip" "$work/ckpt"

echo "exporting" >&2
"$venv/bin/python" scripts/export-deepfilternet-stateful.py \
    --df-src "$src" --checkpoint "$work/ckpt/DeepFilterNet3" \
    --out "$work/out/$model" --check "$upstream" >&2
actual=$(sha256_of "$work/out/$model")
[ "$actual" = "$expected" ] || die "the export differs from the pinned one: expected $expected, got $actual"
cp "$upstream/config.ini" "$work/out/config.ini"
cat >"$work/out/SOURCE" <<SRC
DeepFilterNet3 as one stateful graph, exported by Tessera's
scripts/export-deepfilternet-stateful.py from the pretrained checkpoint in
https://github.com/Rikorose/DeepFilterNet commit $commit (tag v0.5.6),
models/DeepFilterNet3.zip. SHA-256 $expected.
Licence: MIT OR Apache-2.0 (the repository's LICENSE-MIT and LICENSE-APACHE).
SRC
printf '%s' "$expected" >"$work/out/.verified"
rm -rf "$dest"
mv "$work/out" "$dest"
echo "verified and installed into $dest" >&2
printf '%s\n' "$dest"
