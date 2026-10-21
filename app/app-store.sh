#!/bin/bash
#
# MyDiskIsFull Lite for the Mac App Store: builds the App Store edition
# (universal), signs it for the store, wraps it in an installer package,
# has App Store Connect validate it, and with --upload sends it there.
#
#   app/app-store.sh             build/AppStore/MyDiskIsFull-Lite-<version>.pkg, validated
#   app/app-store.sh --upload    and upload it, as a new build of the app
#   BUILD_NUMBER=101 app/app-store.sh --upload
#                                another upload of the same version (each
#                                build number is taken once)
#
# Needs, in release.env (gitignored; see release.env.example):
#   TEAM_ID                the team's id
#   APP_STORE_BUNDLE_ID    the bundle id registered for the store edition
#   APP_STORE_PROFILE      its Mac App Store provisioning profile
#   ASC_KEY_JSON           an App Store Connect API key's details, as JSON
#                          ({"key_id", "issuer_id", "key_filepath"}), or
#                          APPLE_API_KEY_ID and APPLE_API_ISSUER with the key
#                          in ~/.appstoreconnect/private_keys/AuthKey_<id>.p8
# and, in the login keychain, the team's "Apple Distribution" and "Mac
# Installer Distribution" (or "3rd Party Mac Developer Installer") identities.
#
# The app record must exist in App Store Connect first (Apps → New App; the
# API can't create one). The upload adds a build; what the listing says,
# and submitting it for review, happen there.
#
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

say() { printf "\n\033[1;34m==>\033[0m %s\n" "$*"; }
die() { printf "\n\033[1;31mERROR:\033[0m %s\n" "$*" >&2; exit 1; }

UPLOAD=false
for arg in "$@"; do
    case "$arg" in
        --upload) UPLOAD=true ;;
        *) die "unknown option: $arg (the only one is --upload)" ;;
    esac
done

[ -f release.env ] && source release.env
: "${TEAM_ID:?set TEAM_ID in release.env}"
: "${APP_STORE_BUNDLE_ID:?set APP_STORE_BUNDLE_ID in release.env}"
: "${APP_STORE_PROFILE:?set APP_STORE_PROFILE in release.env}"
PROFILE="${APP_STORE_PROFILE/#\~/$HOME}"
[ -f "$PROFILE" ] || die "no provisioning profile at $PROFILE"

# The API key, without ever printing it.
if [ -n "${ASC_KEY_JSON:-}" ]; then
    json="${ASC_KEY_JSON/#\~/$HOME}"
    APPLE_API_KEY_ID="$(plutil -extract key_id raw "$json")"
    APPLE_API_ISSUER="$(plutil -extract issuer_id raw "$json")"
fi
: "${APPLE_API_KEY_ID:?set ASC_KEY_JSON, or APPLE_API_KEY_ID and APPLE_API_ISSUER, in release.env}"
: "${APPLE_API_ISSUER:?set APPLE_API_ISSUER in release.env}"
[ -f "$HOME/.appstoreconnect/private_keys/AuthKey_$APPLE_API_KEY_ID.p8" ] \
    || die "the API key must be at ~/.appstoreconnect/private_keys/AuthKey_<key id>.p8, where altool looks"

identity() {
    security find-identity -v -p basic 2>/dev/null | grep -E "\"($1): .*\\($TEAM_ID\\)\"" | head -1 \
        | sed 's/.*"\(.*\)"/\1/' || true
}
SIGN_APP="$(identity "Apple Distribution")"
SIGN_PKG="$(identity "Mac Installer Distribution|3rd Party Mac Developer Installer")"
[ -n "$SIGN_APP" ] || die "no Apple Distribution identity for team $TEAM_ID in the keychain"
[ -n "$SIGN_PKG" ] || die "no Mac Installer Distribution identity for team $TEAM_ID in the keychain"

# The profile must be for this app and this team.
profile_plist="$(mktemp)"
trap 'rm -f "$profile_plist" "${ENT:-}"' EXIT
security cms -D -i "$PROFILE" > "$profile_plist"
[ "$(plutil -extract Entitlements.com\\.apple\\.application-identifier raw "$profile_plist")" = "$TEAM_ID.$APP_STORE_BUNDLE_ID" ] \
    || die "$PROFILE isn't the Mac App Store profile for $TEAM_ID.$APP_STORE_BUNDLE_ID"

say "Building the App Store edition (universal)"
ARCH=universal app/build.sh --app-store
APP="build/AppStore/MyDiskIsFull Lite.app"
CONTENTS="$APP/Contents"
VERSION="$(plutil -extract CFBundleShortVersionString raw "$CONTENTS/Info.plist")"
BUILD="$(plutil -extract CFBundleVersion raw "$CONTENTS/Info.plist")"
[ "$(plutil -extract CFBundleIdentifier raw "$CONTENTS/Info.plist")" = "$APP_STORE_BUNDLE_ID" ] \
    || die "the bundle id isn't $APP_STORE_BUNDLE_ID"

say "Signing for the App Store as: $SIGN_APP"
cp "$PROFILE" "$CONTENTS/embedded.provisionprofile"
# The store wants the app's own entitlements plus who it is, as the profile
# grants them. The helper keeps exactly the two it inherits the sandbox with.
ENT="$(mktemp).entitlements"
cp app/Resources/AppStore.entitlements "$ENT"
plutil -insert com\\.apple\\.application-identifier -string "$TEAM_ID.$APP_STORE_BUNDLE_ID" "$ENT"
plutil -insert com\\.apple\\.developer\\.team-identifier -string "$TEAM_ID" "$ENT"
xattr -cr "$APP"
codesign --force --timestamp --options runtime --sign "$SIGN_APP" \
    --identifier "$APP_STORE_BUNDLE_ID.mdif" \
    --entitlements app/Resources/AppStore-mdif.entitlements "$CONTENTS/MacOS/mdif"
codesign --force --timestamp --options runtime --sign "$SIGN_APP" --entitlements "$ENT" "$APP"
codesign --verify --deep --strict "$APP"
# A quarantine flag anywhere in the package is rejected after the upload.
if xattr -rl "$APP" | grep -q com.apple.quarantine; then die "a file in the app is quarantined"; fi

PKG="build/AppStore/MyDiskIsFull-Lite-$VERSION-$BUILD.pkg"
say "Packaging, signed as: $SIGN_PKG"
rm -f "$PKG"
productbuild --component "$APP" /Applications --sign "$SIGN_PKG" "$PKG"
pkgutil --check-signature "$PKG" | head -3

KEY=(--apiKey "$APPLE_API_KEY_ID" --apiIssuer "$APPLE_API_ISSUER")
say "Validating with App Store Connect"
xcrun altool --validate-app -f "$PKG" -t macos "${KEY[@]}"

if $UPLOAD; then
    say "Uploading $VERSION ($BUILD)"
    xcrun altool --upload-app -f "$PKG" -t macos "${KEY[@]}"
    say "Uploaded. It shows in App Store Connect once it's processed, usually within the hour."
else
    say "Validated: $PKG. app/app-store.sh --upload sends it."
fi
