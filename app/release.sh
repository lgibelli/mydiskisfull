#!/bin/bash
#
# Packages a release of the full edition, for GitHub:
#
#   app/release.sh            build (universal, Developer ID), notarize and
#                             staple the app; make, sign, notarize and staple
#                             the disk image; sign the Sparkle feed. Into
#                             build/release/:
#                               MyDiskIsFull-<version>.dmg
#                               MyDiskIsFull.dmg     the same, a stable name for
#                                                    .../releases/latest/download/
#                               appcast.xml          what installed copies read
#                               mdif-v<version>-macos-universal.tar.gz (+ .sha256)
#                                                    the command line alone, the
#                                                    same notarized binary
#                               notes.md             this version's CHANGELOG
#   app/release.sh --upload   put those on a *draft* GitHub release v<version>,
#                             for a person to read and publish. Publishing it
#                             is what makes installed copies see the update:
#                             their feed is the latest release's appcast.xml.
#
# Needs (release.env, or the environment in CI; see release.env.example):
#   - TEAM_ID, with its Developer ID Application certificate in the keychain;
#   - notarization: NOTARY_PROFILE (a notarytool keychain profile), or
#     APPLE_API_KEY_PATH, APPLE_API_KEY_ID and APPLE_API_ISSUER;
#   - the Sparkle key: in the login keychain (account below), or
#     SPARKLE_PRIVATE_KEY_FILE.
#
# Each step checks its own result: an exit status of 0 is not evidence that
# a signature or a ticket is there.
#
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP_NAME=MyDiskIsFull
REPO=lgibelli/mydiskisfull
APP="build/$APP_NAME.app"
OUT=build/release

say() { printf "\n\033[1;34m==>\033[0m %s\n" "$*"; }
die() { printf "\n\033[1;31mERROR:\033[0m %s\n" "$*" >&2; exit 1; }

[ -f release.env ] && source release.env
source app/sparkle.sh

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
TAG="v$VERSION"
DMG="$OUT/$APP_NAME-$VERSION.dmg"
CLI="$OUT/mdif-$TAG-macos-universal.tar.gz"

# This version's section of the CHANGELOG ("## [0.1.0] - 2026-10-19").
release_notes() {
    awk -v want="## [$VERSION]" '
        index($0, want) == 1 { inside = 1; next }
        inside && /^## / { exit }
        inside { print }
    ' CHANGELOG.md
}

# LaunchServices registers every app bundle it sees, including copies in a
# staging folder or a mounted image; a record left pointing at a deleted path
# can stop notifications working for the installed copy (Idlewild found out).
# Drop it while the path still exists.
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
ls_unregister() {
    [ -x "$LSREGISTER" ] || return 0
    "$LSREGISTER" -u "$1" 2>/dev/null || true
    local phys
    phys="$(cd "$(dirname "$1")" 2>/dev/null && pwd -P)/$(basename "$1")"
    if [ -n "$phys" ] && [ "$phys" != "$1" ]; then
        "$LSREGISTER" -u "$phys" 2>/dev/null || true
    fi
    return 0
}

have_api_key() {
    [ -n "${APPLE_API_KEY_PATH:-}" ] && [ -n "${APPLE_API_KEY_ID:-}" ] && [ -n "${APPLE_API_ISSUER:-}" ]
}

notarize() {
    if have_api_key; then
        xcrun notarytool submit "$1" --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" \
            --issuer "$APPLE_API_ISSUER" --wait --timeout 30m
    else
        xcrun notarytool submit "$1" --keychain-profile "$NOTARY_PROFILE" --wait --timeout 30m
    fi
}

upload() {
    local files=("$DMG" "$OUT/$APP_NAME.dmg" "$OUT/appcast.xml" "$CLI" "$CLI.sha256")
    for f in "${files[@]}" "$OUT/notes.md"; do
        [ -f "$f" ] || die "$f is missing: run app/release.sh first"
    done
    local built head
    built=$(cat "$OUT/commit")
    head=$(git rev-parse HEAD)
    [ "$built" = "$head" ] || die "build/release was made from $built, not HEAD ($head)"
    git fetch -q origin
    git merge-base --is-ancestor "$head" origin/main 2>/dev/null \
        || git branch -r --contains "$head" | grep -q . \
        || die "push $head first: the release's tag points at it"
    if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
        say "Replacing the files of the existing release $TAG"
        gh release upload "$TAG" --repo "$REPO" --clobber "${files[@]}"
    else
        say "Creating the draft release $TAG"
        gh release create "$TAG" --repo "$REPO" --draft --target "$head" \
            --title "$APP_NAME $VERSION" --notes-file "$OUT/notes.md" "${files[@]}"
    fi
    gh release view "$TAG" --repo "$REPO" --json url,isDraft,assets \
        --jq '"\(.url) (draft: \(.isDraft)): \([.assets[].name] | join(", "))"'
}

case "${1:-}" in
    --upload) upload; exit 0 ;;
    "") ;;
    *) die "unknown option: $1 (the only one is --upload)" ;;
esac

# ---- Preconditions -----------------------------------------------------------

[ -n "${TEAM_ID:-}" ] || die "TEAM_ID is not set (release.env.example says how)"
have_api_key || [ -n "${NOTARY_PROFILE:-}" ] || die "no notarization credentials (NOTARY_PROFILE, or APPLE_API_KEY_*)"
NOTES="$(release_notes)"
[ -n "$NOTES" ] || die "CHANGELOG.md has no section '## [$VERSION]'"
if [ -n "$(git status --porcelain)" ]; then
    die "the working tree has changes: a release is built from a commit"
fi

# ---- 1. The app --------------------------------------------------------------

say "1/5  Build: universal, full edition"
ARCH=universal app/build.sh
IDENTITY=$(codesign -dv "$APP" 2>&1 | sed -n 's/^Authority=\(Developer ID Application: .*\)/\1/p' | head -1)
[ -n "$IDENTITY" ] || die "the app isn't signed with a Developer ID Application certificate"
# Capture, then test: `codesign | grep -q` fails under pipefail exactly when
# grep finds what it looks for.
for bin in "$APP" "$APP/Contents/MacOS/mdif" "$APP/Contents/Frameworks/Sparkle.framework"; do
    flags=$(codesign -dv --verbose=2 "$bin" 2>&1 | grep -o 'flags=0x[0-9a-f]*([^)]*)' || true)
    case "$flags" in
        *runtime*) ;;
        *) die "no hardened runtime on $bin (${flags:-no flags}): notarization would refuse it" ;;
    esac
done

rm -rf "$OUT"
mkdir -p "$OUT"

say "2/5  Notarize and staple the app (usually a few minutes)"
ZIP="$OUT/$APP_NAME-$VERSION.zip"
ditto -c -k --keepParent "$APP" "$ZIP"
notarize "$ZIP"
rm -f "$ZIP"
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"
spctl -a -vv -t execute "$APP"

# The command line on its own, for those who don't want the app: the binary
# from the bundle, so the same signature and notarization ticket.
PKG="$OUT/cli"
mkdir -p "$PKG"
cp "$APP/Contents/MacOS/mdif" README.md LICENSE-MIT LICENSE-APACHE CHANGELOG.md "$PKG/"
tar -C "$PKG" -czf "$CLI" .
rm -rf "$PKG"
(cd "$OUT" && shasum -a 256 "$(basename "$CLI")" > "$(basename "$CLI").sha256")

# ---- 2. The disk image -------------------------------------------------------

say "3/5  Disk image"
STAGE="$OUT/stage"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/$APP_NAME.app"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
ls_unregister "$STAGE/$APP_NAME.app"
rm -rf "$STAGE"
# Gatekeeper assesses the image as well as the app in it.
codesign --force --timestamp --sign "$IDENTITY" "$DMG"
notarize "$DMG"
xcrun stapler staple "$DMG"
xcrun stapler validate "$DMG"

say "4/5  Checking the image as someone downloading it gets it"
MNT=$(mktemp -d)
hdiutil attach "$DMG" -nobrowse -readonly -quiet -mountpoint "$MNT"
CHECK=$(spctl -a -vv -t execute "$MNT/$APP_NAME.app" 2>&1 || true)
STAPLED=yes
xcrun stapler validate "$MNT/$APP_NAME.app" >/dev/null 2>&1 || STAPLED=no
ARCHS=$(lipo -archs "$MNT/$APP_NAME.app/Contents/MacOS/$APP_NAME")
ls_unregister "$MNT/$APP_NAME.app"
hdiutil detach "$MNT" -quiet
rmdir "$MNT" 2>/dev/null || true
echo "$CHECK" | sed 's/^/   /'
case "$CHECK" in
    *"accepted"*"Notarized Developer ID"*) ;;
    *) die "Gatekeeper doesn't accept the app in the image as notarized" ;;
esac
[ "$STAPLED" = yes ] || die "the app in the image has no stapled ticket"
echo "   stapled: yes; architectures: $ARCHS"

# ---- 3. The update feed ------------------------------------------------------

say "5/5  Sparkle feed"
SPARKLE="$(fetch_sparkle)"
FEED="$OUT/feed"
mkdir -p "$FEED"
cp "$DMG" "$FEED/"
printf '%s\n' "$NOTES" > "$OUT/notes.md"
cp "$OUT/notes.md" "$FEED/$APP_NAME-$VERSION.md"
KEYFILE="${SPARKLE_PRIVATE_KEY_FILE:-}"
if [ -z "$KEYFILE" ]; then
    # generate_keys made the keychain item, so it may read it without a
    # prompt (generate_appcast may not): export it for this run only.
    KEYDIR=$(mktemp -d)
    trap 'rm -rf "$KEYDIR"' EXIT
    KEYFILE="$KEYDIR/key"
    "$SPARKLE/bin/generate_keys" --account "$SPARKLE_ACCOUNT" -x "$KEYFILE" >/dev/null
fi
"$SPARKLE/bin/generate_appcast" \
    --ed-key-file "$KEYFILE" \
    --download-url-prefix "https://github.com/$REPO/releases/download/$TAG/" \
    --link "https://github.com/$REPO" \
    --embed-release-notes \
    --maximum-deltas 0 \
    -o "$OUT/appcast.xml" \
    "$FEED"
rm -rf "$FEED"
grep -q 'sparkle:edSignature=' "$OUT/appcast.xml" || die "the appcast has no Ed25519 signature"
grep -q "releases/download/$TAG/$APP_NAME-$VERSION.dmg" "$OUT/appcast.xml" \
    || die "the appcast doesn't point at $TAG's disk image"

cp "$DMG" "$OUT/$APP_NAME.dmg"
git rev-parse HEAD > "$OUT/commit"

printf "\n\033[1;32mDone\033[0m\n"
ls -lh "$OUT" | sed 's/^/   /'
echo "   Next: app/release.sh --upload   (a draft release $TAG, to read and publish)"
