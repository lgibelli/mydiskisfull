#!/bin/bash
#
# Puts the list of what's known about folders online, signed:
#
#   app/knowledge.sh    check crates/mdif-core/knowledge/folders.json, sign it
#                       with the updates' Ed25519 key, check the signature with
#                       the key the app has, and copy both into $KNOWLEDGE_DIR:
#                         folders-1.json       the list (format 1)
#                         folders-1.json.sig   its signature, base64
#
# $KNOWLEDGE_DIR (release.env) is the website's folder that serves
# https://www.salamacchine.it/apps/mydiskisfull/; the site goes live when it's
# pushed. Installed copies download it once a day (`mdif knowledge update`)
# and take it only when its version is higher than the one they have, so the
# file's version must be higher than the one online.
#
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

say() { printf "\n\033[1;34m==>\033[0m %s\n" "$*"; }
die() { printf "\n\033[1;31mERROR:\033[0m %s\n" "$*" >&2; exit 1; }

[ -f release.env ] && source release.env
source app/sparkle.sh

LIST=crates/mdif-core/knowledge/folders.json
[ -n "${KNOWLEDGE_DIR:-}" ] || die "KNOWLEDGE_DIR isn't set (release.env): the website's folder for the list"
[ -d "$KNOWLEDGE_DIR" ] || die "$KNOWLEDGE_DIR isn't a folder"

say "Checking the list"
cargo build --release --offline --quiet -p mdif-cli
MDIF=target/release/mdif
"$MDIF" knowledge check "$LIST"
URL=$("$MDIF" knowledge --json | plutil -extract url raw -o - -)
VERSION=$(plutil -extract version raw -o - "$LIST")
ONLINE=$(curl -sSf --max-time 30 "$URL" 2>/dev/null | plutil -extract version raw -o - - 2>/dev/null || echo 0)
[ "$VERSION" -gt "$ONLINE" ] \
    || die "the list is version $VERSION and $URL is $ONLINE: raise \"version\" in $LIST"

say "Signing version $VERSION"
SPARKLE="$(fetch_sparkle)"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
NAME=$(basename "$URL")
cp "$LIST" "$WORK/$NAME"
KEYFILE="${SPARKLE_PRIVATE_KEY_FILE:-}"
if [ -z "$KEYFILE" ]; then
    # As app/release.sh does: generate_keys made the keychain item, so it may
    # read it without a prompt.
    KEYFILE="$WORK/key"
    "$SPARKLE/bin/generate_keys" --account "$SPARKLE_ACCOUNT" -x "$KEYFILE" >/dev/null
fi
"$SPARKLE/bin/sign_update" --ed-key-file "$KEYFILE" --disable-signing-warning -p "$WORK/$NAME" \
    > "$WORK/$NAME.sig"
rm -f "$WORK/key"
cmp -s "$LIST" "$WORK/$NAME" || die "signing changed the list"
"$MDIF" knowledge check "$WORK/$NAME" --sig "$WORK/$NAME.sig"

cp "$WORK/$NAME" "$WORK/$NAME.sig" "$KNOWLEDGE_DIR/"
printf "\n\033[1;32mDone\033[0m\n"
ls -l "$KNOWLEDGE_DIR/$NAME" "$KNOWLEDGE_DIR/$NAME.sig" | sed 's/^/   /'
echo "   Next: commit and push the website, which puts them at $URL"
