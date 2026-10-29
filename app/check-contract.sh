#!/bin/bash
#
# Decodes everything the app reads, printed by the mdif inside the bundle,
# with the app's own models (app/Sources/Model.swift). A field renamed on
# either side fails here instead of leaving the menu empty.
#
# What a fresh history can't show (open incidents, every kind of fix, an
# undone tweak, a tweak that installs a profile, a tidy-up that ran, alert
# settings that were changed) comes from app/Tests/*.json; Rust tests in
# status_cmd.rs, tweaks_cmd.rs, tidy_cmd.rs and alerts_cmd.rs hold those
# fixtures to the real serialization.
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
"$MDIF" keep add "$ROOT/app" '~/Nothing/Here*' --json --db "$db" > "$T/keep.json"
# Refused (it isn't something mdif suggests), so nothing is touched.
"$MDIF" clean "$T" --json --db "$db" > "$T/clean.json"
"$MDIF" tweaks apply trash-auto-empty --dry-run --json --db "$db" > "$T/apply.json"
"$MDIF" map --json --root "$ROOT/app" --depth 2 --min-mb 0 > "$T/map.json"
# Lists only; and a run of what's on, of which nothing is in a new history,
# as a dry run on top.
"$MDIF" tidy --json --db "$db" > "$T/tidy.json"
"$MDIF" tidy run --dry-run --json --db "$db" > "$T/tidy-run.json"
"$MDIF" alerts --json --db "$db" > "$T/alerts.json"
"$MDIF" timemachine --json --db "$db" > "$T/timemachine.json"
"$MDIF" timemachine --quick --json --db "$db" > "$T/timemachine-quick.json"
# Refused (it isn't a build folder), so nothing is marked.
"$MDIF" timemachine exclude "$T" --json --db "$db" > "$T/exclude.json"

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
    keep="$T/keep.json" \
    clean="$T/clean.json" \
    apply="$T/apply.json" \
    map="$T/map.json" \
    tidy="$T/tidy.json" \
    tidyrun="$T/tidy-run.json" \
    alerts="$T/alerts.json" \
    timemachine="$T/timemachine.json" \
    timemachine="$T/timemachine-quick.json" \
    exclusion="$T/exclude.json" \
    status=app/Tests/status-incidents.json \
    apply=app/Tests/apply-profile.json \
    revert=app/Tests/revert.json \
    suggest=app/Tests/suggest-actions.json \
    space=app/Tests/space.json \
    tidy=app/Tests/tidy.json \
    tidyrun=app/Tests/tidy-run.json \
    alerts=app/Tests/alerts.json \
    timemachine=app/Tests/timemachine.json
