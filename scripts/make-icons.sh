#!/usr/bin/env bash
# Renders Twicecat's icon from its SVG (packaging/icons/io.github.ga_lep.Twicecat.svg)
# into what the packages install: a freedesktop hicolor tree
# (packaging/icons/hicolor/<N>x<N>/apps/<app id>.png and scalable/apps/<app id>.svg)
# and a Windows .ico. The outputs are committed; run this after changing the SVG.
# The macOS .icns is made from the PNGs when the DMG is packaged.
#
# Needs rsvg-convert (librsvg) and ImageMagick 7 (magick).
#
#   scripts/make-icons.sh
set -euo pipefail
cd "$(dirname "$0")/.."

app_id=io.github.ga_lep.Twicecat
svg=packaging/icons/$app_id.svg
out=packaging/icons
for tool in rsvg-convert magick; do
    command -v "$tool" >/dev/null || { echo "error: $tool is not installed" >&2; exit 1; }
done

rm -rf "$out/hicolor"
mkdir -p "$out/hicolor/scalable/apps"
cp "$svg" "$out/hicolor/scalable/apps/$app_id.svg"
for size in 16 24 32 48 64 128 256 512; do
    dir="$out/hicolor/${size}x${size}/apps"
    mkdir -p "$dir"
    rsvg-convert -w "$size" -h "$size" "$svg" -o "$dir/$app_id.png"
done

# Windows: one .ico with the sizes Explorer and the taskbar ask for.
ico_sizes=()
for size in 16 24 32 48 64 256; do
    ico_sizes+=("$out/hicolor/${size}x${size}/apps/$app_id.png")
done
magick "${ico_sizes[@]}" "$out/$app_id.ico"

echo "rendered $out/hicolor and $out/$app_id.ico from $svg" >&2
