#!/bin/bash
#
# Decodes everything the app reads, printed by the mdif inside the bundle,
# with the app's own models (app/Sources/Model.swift). A field renamed on
# either side fails here instead of leaving the menu empty.
#
# What a fresh history can't show (open incidents, every kind of fix, an
# undone tweak, a tweak that installs a profile) comes from app/Tests/*.json;
# Rust tests in status_cmd.rs and tweaks_cmd.rs hold those fixtures to the
# real serialization.
#
#   app/build.sh && app/check-contract.sh
#
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
MDIF="${MDIF:-build/MyDiskIsFull.app/Contents/MacOS/mdif}"
[ -x "$MDIF" ] || { echo "No $MDIF: run app/build.sh first." >&2; exit 1; }

T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
db="$T/history.sqlite3"

"$MDIF" watch --once --db "$db" > /dev/null
"$MDIF" status --json --db "$db" > "$T/status.json"
"$MDIF" forecast --json --series --db "$db" > "$T/forecast.json"
"$MDIF" tweaks --json --db "$db" > "$T/tweaks.json"
"$MDIF" suggest --json --db "$db" > "$T/suggest.json"
"$MDIF" tweaks apply trash-auto-empty --dry-run --json --db "$db" > "$T/apply.json"
"$MDIF" map --json --root "$ROOT/app" --depth 2 --min-mb 0 > "$T/map.json"

# The fixture must have the fields the real command prints, no more, no fewer.
if ! diff <(jq -S 'keys' "$T/status.json") <(jq -S 'keys' app/Tests/status-incidents.json); then
    echo "app/Tests/status-incidents.json and \`mdif status --json\` have different fields" >&2
    exit 1
fi

swiftc -swift-version 5 -target "$(uname -m)-apple-macos14.0" \
    app/Sources/Model.swift app/Tests/Contract.swift -o "$T/contract"
"$T/contract" \
    status="$T/status.json" \
    forecast="$T/forecast.json" \
    tweaks="$T/tweaks.json" \
    suggest="$T/suggest.json" \
    apply="$T/apply.json" \
    map="$T/map.json" \
    status=app/Tests/status-incidents.json \
    apply=app/Tests/apply-profile.json \
    revert=app/Tests/revert.json
