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

### Fixed
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
