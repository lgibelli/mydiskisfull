#!/bin/bash
# The Windows app's update feed, latest.json, for the release of <version>:
# where each installer is on that release, its updater signature (the .sig
# `cargo tauri build` writes beside it when the updater's key is set), and
# the version's section of the CHANGELOG as its notes. release.yml puts it
# on the draft; the app reads it from the latest *published* release
# (tauri.conf.json's plugins > updater), so only publishing ships an update.
#
#   app-windows/feed.sh <version> <folder with both *-setup.exe and .sig>
set -euo pipefail

die() { echo "feed.sh: $*" >&2; exit 1; }
[ $# -eq 2 ] || die "usage: feed.sh <version> <folder>"
version=$1
dir=$2
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://github.com/lgibelli/mydiskisfull/releases/download/v$version"

notes=$(awk -v want="## [$version]" '
    index($0, want) == 1 { inside = 1; next }
    inside && /^## / { exit }
    inside { print }
' "$root/CHANGELOG.md")
[ -n "$notes" ] || die "CHANGELOG.md has no section '## [$version]'"

# The installer for <arch> (Tauri's name: x64, arm64), and its signature.
installer() {
    local file="$dir/MyDiskIsFull_${version}_$1-setup.exe"
    [ -s "$file" ] || die "no $file"
    [ -s "$file.sig" ] || die "no $file.sig: was TAURI_SIGNING_PRIVATE_KEY set?"
    echo "$file"
}
x64=$(installer x64)
arm64=$(installer arm64)

jq -n \
    --arg version "$version" \
    --arg notes "$notes" \
    --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg x64 "$base/$(basename "$x64")" --rawfile x64sig "$x64.sig" \
    --arg arm64 "$base/$(basename "$arm64")" --rawfile arm64sig "$arm64.sig" \
    '{
        version: $version,
        notes: ($notes | sub("^\\s+"; "") | sub("\\s+$"; "")),
        pub_date: $date,
        platforms: {
            "windows-x86_64": { url: $x64, signature: ($x64sig | rtrimstr("\n")) },
            "windows-aarch64": { url: $arm64, signature: ($arm64sig | rtrimstr("\n")) }
        }
    }'
