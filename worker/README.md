# Ask AI's server

The Cloudflare Worker behind MyDiskIsFull's "Ask AI…": where the app's list says nothing about
a folder, the app can ask what it is. It sends only the folder's name (as
`mdif_core::knowledge::generic` makes it: an app's, a tool's or macOS's folder, never one of
your own, nothing inside it), its size rounded to two digits and the macOS version:

```
POST https://ask.salamacchine.it/mydiskisfull/v1/folder
{"name": "~/.cache/codex-runtimes", "bytes": 1700000000, "macos": "26.1"}

200 {"name": "~/.cache/codex-runtimes", "known": true, "what": "…", "grows": "…", "helps": "…",
     "by": "claude-opus-5-5", "cached": false}
```

- The name is checked again here against the same rule (`src/name.ts`), the version is only a
  version and the size only a number, so nothing else can get into the question.
- A name asked before is answered from D1 (`ANSWERS`), so each folder is asked once. What
  Claude couldn't say is asked again after 30 days.
- Otherwise Claude answers (`src/ask.ts`: structured output, with server-side fallbacks), within
  5 questions a minute per Mac (a hash of its IP address, never kept) and `DAILY_ASKS` a day for
  everyone. A command or a link in a text drops that text.
- Nothing about who asked is stored or logged: no invocation logs, no traces.

`400 {"error": "name"}` for a question of another shape, `429 busy`, `503 today` past the day's
cap, `502 ai` when Claude didn't answer.

```sh
npm ci
npm run check   # types (wrangler types, tsc)
npm test        # the name rule, then the Worker under wrangler dev with a stand-in for
                # Anthropic's API; with target/debug/mdif built, the app's own question too
./answers.sh    # what Claude said, most asked first, as entries for folders.json
```

Deploying is in [docs/RELEASING.md](../docs/RELEASING.md#ask-ais-server).
