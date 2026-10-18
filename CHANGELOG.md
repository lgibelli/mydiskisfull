# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## Unreleased

### Added
- `mdif map`: whole-disk map reconciled with what APFS reports in use. Uses APFS private size,
  counts clone families once, keeps cloud-only placeholders separate, and reports purgeable space,
  local Time Machine snapshots and unreadable folders.
- `mdif writers`: which of your processes write to disk, now and since they started.
- A scanner built on `getattrlistbulk` that never follows symlinks, never leaves the volume, never
  enters cloud-only folders, and disables cloud-placeholder downloads for its process.
- The plan: [docs/PLAN.md](docs/PLAN.md).
- `mdif watch`: samples free space and per-process disk writes every 5 minutes into a local
  SQLite history, decimated to 5-minute, hourly and daily points so a year stays under a few MB.
- `mdif forecast`: when the disk will be full, from a Theil–Sen trend over 14 days, 3 days and
  6 hours, with a plausible range. One-off cleanups are taken out of the trend, recurring ones
  (a sawtooth) are kept, and a recent speed-up is reported separately. Also the heaviest writers
  of the last 24 hours and what the watcher itself cost.
- `mdif agent install | uninstall | status`: the watcher as a low-priority LaunchAgent.
- The watcher measures its own CPU time and writes per sample: about 4 ms and 9 KB, so 50 ms
  and 110 KB per hour at the default interval, against a budget of 1 s and 1 MB.
- An end-to-end test with real processes on a scratch APFS image (a filler, a churner, a
  read-only decoy and a one-off cleanup), and a CI job that checks the watcher's budget.

### Security
- File, folder and process names are printed with control characters and bidi overrides escaped
  (`\u{1b}`), so a crafted file name in a scanned folder can't inject terminal escape sequences.
