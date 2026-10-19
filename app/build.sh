#!/bin/bash
#
# Builds build/MyDiskIsFull.app: the Rust watcher and command line, and the
# Swift menu bar app, in one signed bundle. No Xcode project: swiftc and
# cargo assemble it, so the build is reproducible from a shell.
#
#   app/build.sh                 sign with $CODESIGN_IDENTITY, the Developer ID
#                                for TEAM_ID in release.env, or adhoc
#   app/build.sh --app-store     the App Store edition, in build/AppStore: no
#                                tweaks and no per-app writes (the App Sandbox
#                                rules them out), a link to the full version
#                                where they would be, and no Sparkle (the App
#                                Store updates it). See app/Sources/Edition.swift.
#   ARCH=universal app/build.sh  Apple silicon and Intel in one bundle, as
#                                releases are (app/release.sh); arm64 by default
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
ARCH="${ARCH:-arm64}"

say() { printf "\n\033[1;34m==>\033[0m %s\n" "$*"; }
die() { printf "\n\033[1;31mERROR:\033[0m %s\n" "$*" >&2; exit 1; }

EDITION=full
SWIFT_FLAGS=()
OUT=build
for arg in "$@"; do
    case "$arg" in
        --app-store) EDITION="App Store"; SWIFT_FLAGS+=(-D APP_STORE); OUT=build/AppStore ;;
        *) die "unknown option: $arg (the only one is --app-store)" ;;
    esac
done
APP="$OUT/$APP_NAME.app"
CONTENTS="$APP/Contents"

# Account details stay out of the repository, in a gitignored release.env.
[ -f release.env ] && source release.env
source app/sparkle.sh
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
    arm64) ARCHS=(arm64) ;;
    x86_64) ARCHS=(x86_64) ;;
    universal) ARCHS=(arm64 x86_64) ;;
    *) die "ARCH must be arm64, x86_64 or universal" ;;
esac
rust_target() { [ "$1" = arm64 ] && echo aarch64-apple-darwin || echo x86_64-apple-darwin; }

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[ -n "$VERSION" ] || die "no version in Cargo.toml"
IFS=. read -r MAJOR MINOR PATCH <<< "$VERSION"
# Sparkle compares this, so it has to keep increasing.
BUILD=$((MAJOR * 10000 + MINOR * 100 + PATCH))

rm -rf "$APP"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources" "$CONTENTS/Library/LaunchAgents"
SLICES="build/slices"
mkdir -p "$SLICES"

FRAMEWORK_FLAGS=()
if [ "$EDITION" = full ]; then
    SPARKLE="$(fetch_sparkle)"
    FRAMEWORK_FLAGS=(-F "$SPARKLE" -framework Sparkle
                     -Xlinker -rpath -Xlinker @executable_path/../Frameworks)
fi

for arch in "${ARCHS[@]}"; do
    target="$(rust_target "$arch")"
    say "Building mdif ($target)"
    cargo build --release --locked -p mdif-cli --target "$target"

    say "Compiling the app ($arch, $EDITION edition)"
    swiftc -O -swift-version 5 ${SWIFT_FLAGS[@]+"${SWIFT_FLAGS[@]}"} \
        -target "$arch-apple-macos14.0" \
        ${FRAMEWORK_FLAGS[@]+"${FRAMEWORK_FLAGS[@]}"} \
        -framework SwiftUI -framework AppKit -framework Charts \
        -framework UserNotifications -framework ServiceManagement \
        -o "$SLICES/$APP_NAME-$arch" \
        app/Sources/*.swift
done
lipo -create -output "$CONTENTS/MacOS/$APP_NAME" "${ARCHS[@]/#/$SLICES/$APP_NAME-}"
MDIF_SLICES=()
for arch in "${ARCHS[@]}"; do MDIF_SLICES+=("target/$(rust_target "$arch")/release/mdif"); done
lipo -create -output "$CONTENTS/MacOS/mdif" "${MDIF_SLICES[@]}"

cp app/Resources/Info.plist "$CONTENTS/Info.plist"
plutil -replace CFBundleShortVersionString -string "$VERSION" "$CONTENTS/Info.plist"
plutil -replace CFBundleVersion -string "$BUILD" "$CONTENTS/Info.plist"
if [ "$EDITION" != full ]; then
    for key in SUFeedURL SUPublicEDKey SUEnableAutomaticChecks SUScheduledCheckInterval \
               SUVerifyUpdateBeforeExtraction; do
        plutil -remove "$key" "$CONTENTS/Info.plist"
    done
fi
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

if [ "$EDITION" = full ]; then
    mkdir -p "$CONTENTS/Frameworks"
    # ditto, not cp -R: the framework's versioned symlinks must survive.
    ditto "$SPARKLE/Sparkle.framework" "$CONTENTS/Frameworks/Sparkle.framework"
fi

say "Signing as: $IDENTITY"
# Inside out: the helper and the framework first, then the bundle. The
# hardened runtime is what notarization requires of all of them.
STAMP=(--timestamp)
[ "$IDENTITY" = "-" ] && STAMP=(--timestamp=none)
sign() { codesign --force --options runtime "${STAMP[@]}" --sign "$IDENTITY" "$@"; }
sign --identifier "$BUNDLE_ID.mdif" "$CONTENTS/MacOS/mdif"
ENTITLEMENTS=()
if [ "$EDITION" = full ]; then
    sign_sparkle "$CONTENTS/Frameworks/Sparkle.framework" "$IDENTITY"
    if [ "$IDENTITY" = "-" ]; then
        # Library validation (part of the hardened runtime) loads only
        # frameworks signed by the app's own team, and an adhoc signature has
        # none, so Sparkle would never load. Development builds go without it.
        ENTITLEMENTS=(--entitlements "build/adhoc.entitlements")
        cat > build/adhoc.entitlements <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.cs.disable-library-validation</key>
    <true/>
</dict>
</plist>
PLIST
    fi
fi
sign ${ENTITLEMENTS[@]+"${ENTITLEMENTS[@]}"} "$APP"
codesign --verify --deep --strict "$APP"

REQ=$(codesign -d -r- "$APP" 2>&1 | grep designated || true)
if [ "$IDENTITY" = "-" ]; then
    printf "\n\033[1;33mNOTE:\033[0m adhoc signed: Full Disk Access and notifications won't survive a rebuild.\n"
fi
echo "  $REQ"
say "Built: $APP ($VERSION, build $BUILD, $EDITION edition, ${ARCHS[*]})"
