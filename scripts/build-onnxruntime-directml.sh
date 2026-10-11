#!/usr/bin/env bash
# Builds ONNX Runtime at the pinned version with its DirectML provider, for
# the GPU speech enhancement on Windows x64 (plan Task 13b; any DirectX 12
# GPU). Microsoft stopped publishing DirectML builds after 1.24.4 (NuGet
# Microsoft.ML.OnnxRuntime.DirectML, PyPI onnxruntime-directml), but the
# provider is still in the source, so the release tag is built here as
# Microsoft's own build does it: `tools/ci_build/build.py --use_dml`.
#
# Checked: the source is the release tag at the commit below; ONNX
# Runtime's build pins its own dependencies (cmake/deps.txt, SHA-1); the
# DirectML package it would restore from NuGet (Microsoft.AI.DirectML, the
# version its packages.config names) is downloaded here instead, checked
# against the SHA-256 in scripts/onnxruntime.sha256 and put where the
# restore finds it installed; after the build the whole package directory
# must still match it, file for file.
#
# Installed as scripts/fetch-onnxruntime.sh lays a release out (the printed
# lib directory: onnxruntime.dll, its shared-provider library and
# DirectML.dll, with the licences), as the `win-x64-directml` flavour that
# Tessera loads after the CUDA builds and before the CPU one (or from
# TESSERA_ONNXRUNTIME_DIR).
#
# Needs: Windows x64, Visual Studio 2022 or its Build Tools (C++ workload,
# a Windows 10/11 SDK), CMake 3.28 or later, Python 3, git. About 30 GB of
# build tree while it runs, removed once installed (kept when a step fails,
# so the next run reuses the clone and builds incrementally); 20 to 60
# minutes.
#
# Usage: scripts/build-onnxruntime-directml.sh
# Env: TESSERA_ONNXRUNTIME_CACHE overrides the cache root (as
# fetch-onnxruntime.sh); TESSERA_ORT_BUILD_DIR the build tree (default
# <cache root>/b: keep it short, Windows paths stop at 260 characters
# unless long paths are enabled); CMAKE the cmake to use.
# Progress goes to stderr; stdout carries only the path.
set -euo pipefail
cd "$(dirname "$0")/.."

# The release scripts/fetch-onnxruntime.sh pins, and its tag's commit.
version=1.28.3
commit=0d68ff6b3b72b04aac578decd6c4c45d322bb962
dml_version=1.15.4
sums=scripts/onnxruntime.sha256

die() { echo "error: $*" >&2; exit 1; }

sha256_of() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

case "$(uname -s)-$(uname -m)" in
    MINGW*-x86_64 | MSYS*-x86_64 | CYGWIN*-x86_64) ;;
    *) die "DirectML builds are for Windows x64 (this is $(uname -s) $(uname -m))" ;;
esac

package="microsoft.ai.directml.${dml_version}.nupkg"
dml_expected=$(awk -v f="$package" '$2 == f {print $1}' "$sums")
[[ "$dml_expected" =~ ^[0-9a-f]{64}$ ]] || die "no pinned SHA-256 for $package in $sums"

cache_root="${TESSERA_ONNXRUNTIME_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/tessera/onnxruntime}"
dest="$cache_root/$version/win-x64-directml"
marker="$dest/.verified"
pinned="$commit $dml_expected"

if [ -f "$marker" ] && [ "$(cat "$marker")" = "$pinned" ]; then
    printf '%s\n' "$dest/lib"
    exit 0
fi

python=$(command -v python3 || command -v python || command -v py) || die "need Python 3"
cmake=${CMAKE:-$(command -v cmake || true)}
if [ -z "$cmake" ] && [ -x "/c/Program Files/CMake/bin/cmake.exe" ]; then
    cmake="/c/Program Files/CMake/bin/cmake.exe"
fi
[ -n "$cmake" ] || die "need CMake 3.28 or later (or set CMAKE)"

build="${TESSERA_ORT_BUILD_DIR:-$cache_root/b}"
src="$build/ort-src-$version"
out="$build/ort-b"
mkdir -p "$build"

# The tag, checked against its commit (a shallow clone; reused when a
# previous run left it at the same commit).
if [ "$(git -C "$src" rev-parse HEAD 2>/dev/null)" != "$commit" ]; then
    rm -rf "$src"
    echo "cloning ONNX Runtime v$version" >&2
    git -c advice.detachedHead=false clone -q --depth 1 --branch "v$version" \
        https://github.com/microsoft/onnxruntime.git "$src"
    [ "$(git -C "$src" rev-parse HEAD)" = "$commit" ] || die "v$version is not at $commit"
fi
grep -q "id=\"Microsoft.AI.DirectML\" version=\"$dml_version\"" "$src/packages.config" ||
    die "ONNX Runtime v$version does not use DirectML $dml_version"

# The DirectML package, checked.
dml="$build/directml-$dml_version"
if [ ! -f "$dml/.verified" ] || [ "$(cat "$dml/.verified")" != "$dml_expected" ]; then
    rm -rf "$dml"
    mkdir -p "$dml"
    echo "downloading $package" >&2
    curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$dml/$package" \
        "https://api.nuget.org/v3-flatcontainer/microsoft.ai.directml/$dml_version/$package"
    actual=$(sha256_of "$dml/$package")
    [ "$actual" = "$dml_expected" ] || die "SHA-256 mismatch for $package: expected $dml_expected, got $actual"
    "$python" -m zipfile -e "$dml/$package" "$dml/x"
    printf '%s' "$dml_expected" >"$dml/.verified"
fi

# Installed where the build's NuGet restore looks (a packages.config
# restore skips a package whose directory holds its .nupkg), so nothing
# unchecked is downloaded or compiled against.
restored="$out/packages/Microsoft.AI.DirectML.$dml_version"
rm -rf "$restored"
mkdir -p "$restored"
cp -R "$dml/x/." "$restored/"
cp "$dml/$package" "$restored/Microsoft.AI.DirectML.$dml_version.nupkg"

echo "building ONNX Runtime $version with DirectML (20 to 60 minutes)" >&2
# CMake 4 refuses projects that ask for a CMake older than 3.5 (some of
# ONNX Runtime's fetched dependencies do); harmless with CMake 3.
CMAKE_POLICY_VERSION_MINIMUM=3.5 "$python" "$src/tools/ci_build/build.py" \
    --build_dir "$(cygpath -w "$out")" --config Release --build_shared_lib \
    --parallel --use_dml --skip_tests --skip_submodule_sync \
    --compile_no_warning_as_error --cmake_path "$(cygpath -w "$cmake")" \
    --cmake_extra_defines onnxruntime_BUILD_UNIT_TESTS=OFF >&2

bin="$out/Release/Release"
cmp -s "$restored/Microsoft.AI.DirectML.$dml_version.nupkg" "$dml/$package" ||
    die "the build replaced the DirectML package"
diff -r -q -x "*.nupkg" "$dml/x" "$restored" >&2 || die "the DirectML the build used differs from $package"
cmp -s "$bin/DirectML.dll" "$dml/x/bin/x64-win/DirectML.dll" || die "DirectML.dll next to the build differs from $package"

mkdir -p "$cache_root/$version"
work=$(mktemp -d "$cache_root/$version/.build.XXXXXX")
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/out/lib" "$work/out/licenses"
for library in onnxruntime.dll onnxruntime_providers_shared.dll DirectML.dll; do
    [ -f "$bin/$library" ] || die "$library not built"
    cp "$bin/$library" "$work/out/lib/"
done
cp "$src/LICENSE" "$src/ThirdPartyNotices.txt" "$work/out/"
cp "$dml/x/LICENSE.txt" "$work/out/licenses/DirectML-LICENSE.txt"
cp "$dml/x/ThirdPartyNotices.txt" "$work/out/licenses/DirectML-ThirdPartyNotices.txt"
printf '%s' "$pinned" >"$work/out/.verified"
rm -rf "$dest"
mv "$work/out" "$dest"
rm -rf "$src" "$out" "$dml"
rmdir "$build" 2>/dev/null || true
echo "built and installed into $dest" >&2
printf '%s\n' "$dest/lib"
