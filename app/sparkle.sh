# Sparkle, the update framework of the full edition. Source this file from
# app/build.sh and app/release.sh, don't run it.
#
# The App Store updates its own apps, so only the full edition (from GitHub)
# carries Sparkle. There's no Xcode project, so the framework comes from the
# official release tarball, which is how Sparkle's documentation says to use
# it without Xcode. The tarball is pinned by SHA-256: the framework inside is
# adhoc signed, so there's no upstream signature to check, and every piece is
# re-signed with our identity (the hardened runtime won't load a framework
# signed by another team).

SPARKLE_VERSION="2.10.0"
SPARKLE_SHA256="c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c"
SPARKLE_VENDOR="$ROOT/build/vendor/sparkle-$SPARKLE_VERSION"

# The Ed25519 key that signs updates lives in the login keychain under this
# account (`generate_keys --account`), apart from other apps' keys.
SPARKLE_ACCOUNT="io.github.lgibelli.mydiskisfull"

# Download, check and unpack the distribution; prints the directory holding
# Sparkle.framework and bin/.
fetch_sparkle() {
    if [ ! -d "$SPARKLE_VENDOR/Sparkle.framework" ]; then
        local tarball="$ROOT/build/vendor/Sparkle-$SPARKLE_VERSION.tar.xz"
        mkdir -p "$SPARKLE_VENDOR"
        if [ ! -f "$tarball" ]; then
            curl -sSL --fail -o "$tarball" \
                "https://github.com/sparkle-project/Sparkle/releases/download/$SPARKLE_VERSION/Sparkle-$SPARKLE_VERSION.tar.xz" \
                || die "could not download Sparkle $SPARKLE_VERSION"
        fi
        local got
        got=$(shasum -a 256 "$tarball" | cut -d' ' -f1)
        [ "$got" = "$SPARKLE_SHA256" ] || die "Sparkle $SPARKLE_VERSION tarball has the wrong hash.
       expected $SPARKLE_SHA256
       got      $got"
        # Only what ships and what signs. The distribution also has a test app,
        # which LaunchServices would register from build/.
        tar -xf "$tarball" -C "$SPARKLE_VENDOR" \
            ./Sparkle.framework ./bin ./LICENSE ./CHANGELOG \
            || die "could not unpack $tarball"
    fi
    echo "$SPARKLE_VENDOR"
}

# sign_sparkle <framework> <identity>: inside out, Sparkle's own executables
# and services first, keeping their entitlements, then the framework. All with
# the hardened runtime, which notarization requires of nested code too.
sign_sparkle() {
    local fw="$1" identity="$2" item
    local stamp=(--timestamp)
    [ "$identity" = "-" ] && stamp=(--timestamp=none)
    for item in "$fw/Versions/B/Autoupdate" \
                "$fw/Versions/B/Updater.app" \
                "$fw"/Versions/B/XPCServices/*.xpc; do
        [ -e "$item" ] || continue
        codesign --force --options runtime "${stamp[@]}" \
            --preserve-metadata=entitlements --sign "$identity" "$item" >/dev/null
    done
    codesign --force --options runtime "${stamp[@]}" --sign "$identity" "$fw" >/dev/null
}
