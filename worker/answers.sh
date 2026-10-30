#!/bin/bash
#
# What Claude said of folders, most asked first, as entries for
# crates/mdif-core/knowledge/folders.json:
#
#   worker/answers.sh            the Worker's database (your Cloudflare login)
#   worker/answers.sh --local    the one `wrangler dev` uses
#
# Check each against what the app or tool says of it before it goes in (add
# "name" and "source"; a Library/<kind>/<bundle id> folder is better as an
# "app" entry), raise the list's version, and publish it (docs/RELEASING.md).
# What's in the list is then said on every Mac, and no longer asked.
#
set -euo pipefail
cd "$(dirname "$0")"
WHERE=--remote
[ "${1:-}" = --local ] && WHERE=--local
./node_modules/.bin/wrangler d1 execute ANSWERS "$WHERE" --json --command \
    "SELECT name, what, grows, helps, asked, macos FROM answers WHERE known = 1 ORDER BY asked DESC LIMIT 100" \
| python3 -I -c '
import json, sys
rows = json.load(sys.stdin)[0]["results"]
for r in rows:
    name = r["name"]
    entry = {"path": name[2:] if name.startswith("~/") else name}
    for k in ("what", "grows", "helps"):
        if r[k]:
            entry[k] = r[k]
    asked, macos = r["asked"], r["macos"]
    print(f"// asked {asked} times, first on macOS {macos}")
    print(json.dumps(entry, ensure_ascii=False, indent=2) + ",")
print(f"// {len(rows)} answers", file=sys.stderr)
'
