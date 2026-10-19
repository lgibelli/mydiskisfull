#!/bin/bash
#
# Builds build/MyDiskIsFull.app: the Rust watcher and command line, and the
# Swift menu bar app, in one signed bundle. No Xcode project: swiftc and
# cargo assemble it, so the build is reproducible from a shell.
#
#   app/build.sh                 sign with $CODESIGN_IDENTITY, the Developer ID
#                                for TEAM_ID in release.env, or adhoc
#
# An adhoc signature is a hash of the binary, so macOS treats every rebuild
# as a new app: Full Disk Access and notification permission don't survive
# it. A Developer ID (or Apple Development) signature keeps them.
#
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP_NAME=MyDiskIsFull
BUNDLE_ID=io.github.lgibelli.mydiskisfull
APP="build/$APP_NAME.app"
CONTENTS="$APP/Contents"
ARCH="${ARCH:-arm64}"

say() { printf "\n\033[1;34m==>\033[0m %s\n" "$*"; }
die() { printf "\n\033[1;31mERROR:\033[0m %s\n" "$*" >&2; exit 1; }

# Account details stay out of the repository, in a gitignored release.env.
[ -f release.env ] && source release.env
identity() {
    if [ -n "${CODESIGN_IDENTITY:-}" ]; then echo "$CODESIGN_IDENTITY"; return; fi
    if [ -n "${TEAM_ID:-}" ]; then
        local found
        found=$(security find-identity -v -p codesigning 2>/dev/null \
                | grep "Developer ID Application: .*($TEAM_ID)" | head -1 \
                | sed 's/.*"\(.*\)"/\1/' || true)
        if [ -n "$found" ]; then echo "$found"; return; fi
    fi
    echo "-"
}
IDENTITY="$(identity)"

case "$ARCH" in
    arm64) RUST_TARGET=aarch64-apple-darwin ;;
    x86_64) RUST_TARGET=x86_64-apple-darwin ;;
    *) die "ARCH must be arm64 or x86_64" ;;
esac

say "Building mdif ($RUST_TARGET)"
cargo build --release --locked -p mdif-cli --target "$RUST_TARGET"

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[ -n "$VERSION" ] || die "no version in Cargo.toml"
IFS=. read -r MAJOR MINOR PATCH <<< "$VERSION"
BUILD=$((MAJOR * 10000 + MINOR * 100 + PATCH))

rm -rf "$APP"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources" "$CONTENTS/Library/LaunchAgents"

say "Compiling the app ($ARCH)"
swiftc -O -swift-version 5 \
    -target "$ARCH-apple-macos14.0" \
    -framework SwiftUI -framework AppKit -framework Charts \
    -framework UserNotifications -framework ServiceManagement \
    -o "$CONTENTS/MacOS/$APP_NAME" \
    app/Sources/*.swift

cp "target/$RUST_TARGET/release/mdif" "$CONTENTS/MacOS/mdif"
cp app/Resources/Info.plist "$CONTENTS/Info.plist"
plutil -replace CFBundleShortVersionString -string "$VERSION" "$CONTENTS/Info.plist"
plutil -replace CFBundleVersion -string "$BUILD" "$CONTENTS/Info.plist"
cp "app/Resources/$BUNDLE_ID.watcher.plist" "$CONTENTS/Library/LaunchAgents/"
plutil -lint "$CONTENTS/Info.plist" "$CONTENTS/Library/LaunchAgents/$BUNDLE_ID.watcher.plist" >/dev/null

ICONSET="build/$APP_NAME.iconset"
if [ ! -f "build/$APP_NAME.icns" ] || [ app/make-icon.swift -nt "build/$APP_NAME.icns" ]; then
    say "Drawing the icon"
    rm -rf "$ICONSET"
    swift app/make-icon.swift "$ICONSET"
    iconutil -c icns "$ICONSET" -o "build/$APP_NAME.icns"
    rm -rf "$ICONSET"
fi
cp "build/$APP_NAME.icns" "$CONTENTS/Resources/"

say "Signing as: $IDENTITY"
# Inside out: the helper first, then the bundle. The hardened runtime is what
# notarization will require of both.
sign() {
    local stamp=(--timestamp)
    [ "$IDENTITY" = "-" ] && stamp=(--timestamp=none)
    codesign --force --options runtime "${stamp[@]}" --sign "$IDENTITY" "$@"
}
sign --identifier "$BUNDLE_ID.mdif" "$CONTENTS/MacOS/mdif"
sign "$APP"
codesign --verify --deep --strict "$APP"

REQ=$(codesign -d -r- "$APP" 2>&1 | grep designated || true)
if [ "$IDENTITY" = "-" ]; then
    printf "\n\033[1;33mNOTE:\033[0m adhoc signed: Full Disk Access and notifications won't survive a rebuild.\n"
fi
echo "  $REQ"
say "Built: $APP ($VERSION, build $BUILD)"
