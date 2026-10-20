# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## Unreleased

### Changed
- A redesigned app. A main window with a sidebar and four pages: Overview (a capacity ring and
  the forecast in a sentence, led by whatever would fill the disk soonest; what needs attention,
  each with its fix; the last days; the biggest fixes), Trends (free space with the forecast and
  its plausible range, the alert floor, cleanups, readings under the pointer; what grew and wrote
  most), Disk Map (a storage bar, then the sunburst, treemap or list) and Fixes (suggestions and
  the tweaks as switches). ⌘1 to ⌘4 switch pages.
- The menu bar shows a glance instead of a long menu: free space, the forecast, the last three
  days and what needs attention, with the way into the window. The app joins the Dock while a
  window is open.
- A new icon, Settings as a grouped form, a new welcome, and the disk map coloured by folder by
  default (growth is one click away).

## [0.1.0] - 2026-10-19

The first release.

### Added
- Releases: a universal (Apple silicon and Intel) disk image, signed with a Developer ID,
  notarized and stapled, and the command line on its own. `app/release.sh` makes them and
  checks each step (hardened runtime, the tickets, Gatekeeper on the image as downloaded);
  `app/release.sh --upload` puts them on a draft GitHub release. The release workflow does the
  same from a `release/x.y.z` branch and attests the provenance of what it built.
- Updates with Sparkle: once a day the app reads the appcast attached to the latest GitHub
  release, and installs an update only if it carries the project's Ed25519 signature and is
  signed by the same team. "Check for Updates…" in the menu, a switch in Settings, a line in
  About. A reminder notification when an update waits behind other windows. After an update
  the app restarts the watcher, which would otherwise keep running the old `mdif`. The App
  Store edition has none of it: the App Store updates its own apps.
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
- The watcher measures its own CPU time and writes: samples and folder updates apart (an hour is
  12 of one and 2 of the other at the defaults), and the first folder update after a start, which
  catches up on everything changed while it wasn't running, on its own. On a busy development
  Mac that came to about 0.65 s and 0.4 MB an hour, against a budget of 1 s and 1 MB.
- An end-to-end test with real processes on a scratch APFS image (a filler, a churner, a
  read-only decoy and a one-off cleanup), and a CI job that checks the watcher's budget.
- Growth per folder (`mdif owners`). Every 30 minutes the watcher replays the FSEvents journal
  from where it left off (so time it wasn't running is covered too), re-lists only the folders
  that changed, and adds the folders of files that heavy writers keep open (FSEvents doesn't
  report a file while it is written, only when it's closed). Folders are grouped into owners:
  app containers, caches and support folders, projects with their build output and worktrees
  separate, top-level home folders. Busy folders with thousands of entries are re-listed less
  often so the watcher stays in budget. Cleanups are remembered with what came back since.
- Incidents (`mdif incidents`): a folder that keeps growing (steady fit, ≥ 1 GB and ≥ 5% of
  its size, still growing, would fill the disk within 7 days, not allowlisted) or a process
  that keeps rewriting data (≥ 1 GB in an hour, steadily, while the disk kept < 10%), each in
  one sentence. Episodes open once and close when the clauses stop holding; the agent logs new
  ones and `mdif forecast` shows open ones first, with the folders that grew most.

- Suggestions (`mdif suggest`): what to do about each cause so it stays fixed, from the
  watcher's history (open incidents, folders that grew, cleanups that refilled) and from a look
  at the disk (what a tweak would stop, and when space is tight: local Time Machine snapshots,
  swap, Docker's disk image). Incidents now end with their fix, in `mdif incidents`,
  `mdif forecast` and the agent's log. Commands are printed for you to run, with paths
  shell-quoted; a process name never goes into a command.
- Tweaks (`mdif tweaks`, `mdif tweaks apply <id> [--dry-run]`, `mdif tweaks revert <id>`):
  Apple Intelligence off, Chrome's on-device AI model off and its cache capped (as configuration
  profiles you approve in System Settings), Gradle and Homebrew keeping unused downloads for less
  long, Claude Code keeping transcripts for 14 days, and the Trash emptied after 30 days. Applying
  records what each one replaces, reverting puts it back (but keeps what you changed since), and
  `mdif tweaks` shows whether it worked: the setting still holds, the restart happened, and its
  folders shrank or grew slower than before.
- `mdif writers` names the disk image `diskimages-helper` is writing to, since those bytes are
  its clients' writes.
- Churn by `cfprefsd` is said for what it is, an app saving its settings over and over, and its
  fix names the app whose large preferences file was just rewritten.

- The menu bar app (`app/`, SwiftUI, built with `app/build.sh`): free space and the forecast in
  the menu, open incidents with their fix, suggestions, and the tweaks as checkmarks that apply or
  undo after showing what they change. An overview window with a gauge and a free-space chart
  with the forecast, the disk map (a sunburst, a treemap coloured by each folder's growth in the
  last day like a market heatmap, or a list; click to zoom in), which keeps itself current
  while it's open: measured again on opening if the last map is over 30 minutes old and every
  hour while it stays open, at background priority, the previous map showing meanwhile. Never
  in the background, though: one whole-disk map takes about 200 s of CPU, 200 times the
  watcher's budget for an hour. Settings, a first-launch
  welcome and an About panel with the watcher's measured cost. Notifications for a new
  incident, the forecast crossing 14, 3 and 1 days or free space under 10 GB, and a tweak
  something undid, once per episode. The watcher runs from the bundle as an SMAppService
  LaunchAgent (its own label, `…mydiskisfull.watcher`) and takes over from `mdif agent install`.
- `app/build.sh --app-store`: an App Store edition of the app, without the tweaks and the
  per-app writes (the App Sandbox rules them out; their code isn't compiled in). Where they
  would be, it says why and links the free, open-source full version. Not sandboxed yet.
- `mdif status`: free space, the forecast, an alert level, open incidents, tweaks something undid
  and whether the watcher has Full Disk Access. With `--json` it is what the app reads; it only
  reads the history, so it takes milliseconds.
- `mdif forecast --json --series` (the free-space history), `--json` for `mdif tweaks apply` and
  `mdif tweaks revert`, and `mdif watch --log` (errors to `~/Library/Logs/MyDiskIsFull`).
- The watcher records whether it has Full Disk Access at every sample, and what each folder
  update listed (folders, entries, CPU, the biggest folders), which `mdif forecast` shows.
- `mdif status` judges a tweak it applied by the settings its record names, not by where they
  would be looked for now, so the app (without the shell's `GRADLE_USER_HOME` or
  `XDG_CONFIG_HOME`) doesn't take it for undone.
- `app/check-contract.sh`: decodes the real output of every command the app uses with the app's
  own models, in CI.

### Fixed
- The watcher was over its budget on a busy Mac: its folder updates came to about 2 s of CPU an
  hour, two thirds of it re-listing five folders (two builds' `deps`, Chrome's and Slack's disk
  caches, a log folder). A folder whose size hardly changes between listings (under a fifth of
  the slowest growth an incident can have, 16 MB a period) now waits twice, then four times as
  long to be listed again (8 hours at most for the biggest), and an update lists at most 20,000
  entries of folders it has seen before, the most overdue first. On the same journal that is
  0.5–0.8 s an hour, 0.4 s at most. What a folder grew while it waited still counts in full.
- The watcher could stop for as long as a privacy prompt stayed unanswered: without Full Disk
  Access, opening Documents, Desktop, Downloads, iCloud Drive, cloud storage, another app's
  container or another volume shows a dialog, and the open waits for it (18 minutes on the
  development Mac). The watcher and `mdif status` now leave those folders alone until they have
  Full Disk Access.
- `mdif forecast --json` printed prose when there was no history yet.
- `mdif agent install` refuses to start a second watcher next to the app's, and `mdif agent
  status` shows either.
- Modified clones were counted twice: once one copy changes it gets its own clone ID while still
  sharing most blocks. Files that may share blocks are now measured by their physical extents
  (`F_LOG2PHYS_EXT`), so shared blocks count once; files whose layout can't be read are reported
  as an explicit uncertainty instead.
- Hard links were counted once per name. A hard-linked file now counts once, and its space is only
  reclaimable when all its links are inside the scanned folder ("linked out" otherwise).
- Small sparse files were under-counted: APFS reserves their whole length while their private
  size covers only what was written. They are counted at their allocated size.
- The forecast weighted the last day 12× more than older days because recent history is kept at
  5-minute resolution and older history hourly, so compaction alone could change it (1.1 →
  10 GB/day in one case). Each window is now resampled evenly in time before fitting.
- Watching a path that isn't valid UTF-8 with FSEvents aborted the process; it is now an error.

### Security
- File, folder and process names are printed with control characters and bidi overrides escaped
  (`\u{1b}`), so a crafted file name in a scanned folder can't inject terminal escape sequences.

[0.1.0]: https://github.com/lgibelli/mydiskisfull/releases/tag/v0.1.0
