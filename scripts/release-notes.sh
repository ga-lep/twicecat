#!/usr/bin/env bash
# Prints the top of a release's notes: a table of what to download for each
# system, with direct links to the files scripts/release-names.sh named, then
# how to check them and whether they are signed. The release workflow puts
# GitHub's generated changelog under it.
#
#   scripts/release-notes.sh <tag> <owner/repo> <windows signed> <macos signed>
#   (each "signed" is true or false)
set -euo pipefail

[ $# -eq 4 ] || { echo "usage: $0 <tag> <owner/repo> <windows signed> <macos signed>" >&2; exit 2; }
tag=$1 repo=$2 windows_signed=$3 macos_signed=$4
v=${tag#v}
url="https://github.com/$repo/releases/download/$tag"
file() { echo "$url/twicecat-$v-$1"; }
linux() {
    local arch=$1
    echo "[Flatpak]($(file "linux-$arch.flatpak")) (recommended) · [AppImage]($(file "linux-$arch.AppImage")) · [.deb]($(file "linux-$arch.deb")) (Ubuntu, Debian) · [.rpm]($(file "linux-$arch.rpm")) (Fedora) · [.tar.zst]($(file "linux-$arch.tar.zst"))"
}

cat <<EOF
## Downloads

| System | Download |
|---|---|
| **Windows** (x86_64) | [Installer (.msi)]($(file windows-x86_64.msi)) |
| **macOS** (Apple Silicon) | [Disk image (.dmg)]($(file macos-arm64.dmg)) |
| **Linux** (x86_64: Intel, AMD) | $(linux x86_64) |
| **Linux** (arm64, such as the Steam Frame) | $(linux arm64) |

On Linux, the Flatpak works on any distribution with Flatpak. The AppImage is a single file to run, the .deb and .rpm install with your package manager, and the .tar.zst unpacks anywhere. The \`.debug-symbols.tar.zst\` files are only for reading crash reports.

Check a download against [SHA256SUMS.txt]($url/SHA256SUMS.txt), or with \`gh attestation verify <file> --repo $repo\`.

EOF
if [ "$windows_signed" = true ]; then echo "- Windows: signed."; else echo "- Windows: **unsigned**; SmartScreen warns on first start."; fi
if [ "$macos_signed" = true ]; then echo "- macOS: signed with a Developer ID and notarized."; else echo "- macOS: **unsigned**; macOS refuses to open it until you allow it in System Settings → Privacy & Security."; fi
echo "- Linux: packages are not signed; SHA256SUMS.txt and the attestations cover them."
