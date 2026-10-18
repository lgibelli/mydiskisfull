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

### Security
- File, folder and process names are printed with control characters and bidi overrides escaped
  (`\u{1b}`), so a crafted file name in a scanned folder can't inject terminal escape sequences.
