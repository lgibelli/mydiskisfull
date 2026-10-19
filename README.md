# MyDiskIsFull

A macOS app that watches your disk over time, **predicts when it will be full**, catches the
**runaway disk fillers** responsible, says *why* in one sentence, and tells you **how to stop it
from happening again**.

Disk analyzers show you the current picture once you go and look. MyDiskIsFull watches all the
time and speaks up when something is filling the disk fast enough to matter:

> Full in about 19 days at the current rate (3.1 GB/day). 1.6 GB/day is one project's Rust build;
> clearing it frees 11 GB, and sharing one build folder across its worktrees stops it coming back.

> OneDrive is rewriting 3 GB of logs every 10 minutes while uploading nothing, usually a stuck
> sync. Restarting OneDrive fixes it.

> Time Machine hasn't completed a backup since Tuesday. 6 local snapshots now hold 31 GB, growing
> 8 GB/day; at this rate the disk is full on Saturday.

Free and open source, with no telemetry and no account.

> **Status: early development.** The watcher and its forecast (`mdif watch`,
> `mdif forecast`), growth per folder and runaway-filler incidents (`mdif owners`,
> `mdif incidents`), what to do about each cause (`mdif suggest`) and reversible tweaks that stop
> things coming back (`mdif tweaks`), the honest disk map (`mdif map`) and the disk-writer monitor
> (`mdif writers`) work today, in a menu bar app and on the command line. 0.1.0 is the first
> release; what's next is in the [plan](docs/PLAN.md).

## Why another disk tool

The project started from a real cleanup of a 494 GB Mac with 6 GB free. Most of what filled it
was invisible to snapshot tools:

- A stuck sync client wrote about 18 GB/hour of logs while its folder size stayed flat.
- A cache cleared on Monday was back to 4 GB by Tuesday.
- Agent-created git worktrees held 36 GB of build output.
- A Time Machine local snapshot kept 26 GB of deleted files allocated.
- `du` double-counted APFS clones, so even the "current picture" was wrong.

## Design constraints

- **A monitor must never become the thing it hunts.** Under 1 s of CPU per hour, under 1 MB/hour
  of its own writes, and history capped at 20 MB, all measured and shown in the app.
- **False positives are the whole product.** Growth is only an incident when a fitted trend holds
  (r² ≥ 0.85 over hours), it has grown by gigabytes, and it would fill the disk within the
  horizon. Builds, imports and backups that stop don't count.
- **Honest numbers.** It measures APFS *private size* (what deleting would actually free), counts
  every block shared by clones (modified ones too, by their physical extents) and every hard link
  once, and keeps cloud-only placeholders separate. The total reconciles with what APFS reports
  in use, and what it can't pin down is shown as uncertainty.
- **Never costs you network traffic.** Scanning turns off cloud-placeholder downloads for the
  process, so looking at an iCloud or OneDrive folder never downloads it.
- **Prove it before deleting.** Only regenerable data, or data with a verified copy, is offered
  for deletion. Personal files only ever go through a verified move to your cloud or an archive.

## Install

Download **MyDiskIsFull.dmg** from the
[latest release](https://github.com/lgibelli/mydiskisfull/releases/latest), open it and drag
MyDiskIsFull into Applications. It needs macOS 14 or later, on Apple silicon or Intel, and it's
signed and notarized. It updates itself with [Sparkle](https://sparkle-project.org): once a day
it reads the latest release's feed from GitHub (nothing is sent), and an update installs only if
it carries the project's signature. Settings turns the check off.

The command line is inside the app; to use it from a terminal:

```sh
sudo mkdir -p /usr/local/bin
sudo ln -sf /Applications/MyDiskIsFull.app/Contents/MacOS/mdif /usr/local/bin/mdif
```

or download `mdif-…-macos-universal.tar.gz` from the same release on its own.

## Build it

Requires macOS and Rust 1.88+; the menu bar app needs macOS 14 and Xcode's command line tools.

### The menu bar app

```sh
git clone https://github.com/lgibelli/mydiskisfull.git
cd mydiskisfull
app/build.sh                  # build/MyDiskIsFull.app: the app, with mdif inside
open build/MyDiskIsFull.app
app/build.sh --app-store      # the App Store edition, in build/AppStore (see below)
```

The first launch starts the watcher (a LaunchAgent inside the app running the same `mdif watch`
as below; one started with `mdif agent install` is stopped, and the history carries on), asks for
Full Disk Access and for notifications. The menu shows free space and when the
disk will be full, what is filling it with the fix for each, suggestions, and the tweaks, which
apply and undo with a click after saying what they change and what you give up. The overview
window charts free space with the forecast. The disk map runs when you ask for it and shows the
space as a sunburst, as a treemap like a stock-market heatmap (tile size is space, colour is how
much the folder grew or shrank in the last day) or as a list; click a folder to zoom in.
Notifications come once per episode: a runaway filler, the disk full within two weeks, three days
or a day, and a tweak something undid.

Without Full Disk Access macOS keeps the watcher out of other apps' folders and your Desktop,
Documents and Downloads. It leaves them alone rather than ask, since a background process asking
would wait on a dialog nobody may be there to answer; they are simply not watched until you grant
it.

To keep that permission across rebuilds, sign the app with your own certificate: copy
`release.env.example` to `release.env` and set your team ID. Unsigned (adhoc) builds work, but
macOS forgets their permissions every time you rebuild. Making a release is in
[docs/RELEASING.md](docs/RELEASING.md).

### The command line

```sh
cargo build --release
./target/release/mdif status           # free space, the forecast and what needs attention
./target/release/mdif map              # where the space is, reconciled with APFS
./target/release/mdif map --root ~     # just your home folder
./target/release/mdif writers          # which processes are writing to disk right now

./target/release/mdif agent install    # start the watcher in the background (one sample / 5 min)
./target/release/mdif forecast         # when the disk will be full, once there's some history
./target/release/mdif owners           # which folders grew, and what came back after cleanups
./target/release/mdif incidents        # runaway fillers: folders that keep growing, churning writers
./target/release/mdif suggest          # what to do about each cause, so it stays fixed
./target/release/mdif tweaks           # settings that stop things coming back, applied and undone
./target/release/mdif agent status
```

The forecast needs a few hours of history to say anything, and gets better over days. Illustrative
output after a few days:

```
Free now: 38.2 GB on /System/Volumes/Data  (Oct 18 16:05)

Filling at 3.1 GB a day (3-day trend from 790 samples, plausibly 2.4–3.9 GB a day).
Full in about 12 days, around Oct 30 (between Oct 28 and Nov 3).
Space that came back in one go, taken out of the trend (one-off): +21.4 GB on Oct 16 11:20.

Most written to disk in the last 24 hours:
     41 GB  OneDrive
    6.2 GB  rustc
    1.1 GB  Google Chrome Helper

Watcher cost: 5.0 ms CPU and 33 KB written per sample, and 300 ms for each update of the folders
(one every 30 min). At one sample every 5 min, that is 660 ms and 396 KB per hour (within budget:
1 s and 1 MB).
The first update after it started took 1900 ms: it caught up on what changed while the watcher
wasn't running.
```

Those numbers are from a busy development Mac with several builds running at once; a quiet Mac
costs a fraction of that.

Every half hour the watcher also works out which folders grew. It replays the FSEvents journal
since its last look, re-lists only the folders that changed (one level each, never the whole
disk), and asks the processes that wrote the most which files they have open, because a log that
is kept open while it grows never shows up in FSEvents. Growth is attributed to folders a person
recognises: an app's container or cache, a project and, separately, its build output or one of
its worktrees. When a folder keeps growing or a process keeps rewriting data, and every clause in
the [plan](docs/PLAN.md#2-runaway-fillers) holds, it becomes an incident:

```
[growth] since Oct 18 14:30 (still going)
  ~/Library/Logs/SomeApp grew 4.8 GB in 6 hours (19 GB a day, steadily, now 11 GB). At that
  rate the disk is full in 2 days.
[churn] since Oct 18 15:05 (still going)
  OneDrive wrote 3.1 GB in 60 minutes while the disk kept 1%: it is rewriting the same data
  (74 GB a day of SSD wear). A sync or log that is stuck is the usual cause; restarting it
  usually stops it.
```

The history lives in `~/Library/Application Support/MyDiskIsFull/history.sqlite3` and never
leaves the Mac.

### Fixes that last

Deleting something once is rarely the answer: caches refill, models download again, builds come
back. `mdif suggest` lists what is worth doing, from the watcher's history (incidents, folders
that grew, cleanups that refilled) and from a look at the disk, each with the fix that keeps it
from coming back (illustrative):

```
 1. Gradle's caches take 8.8 GB. Gradle keeps downloaded dependencies and Gradle distributions
    for 30 days after their last use, and every Gradle version has a cache of its own.
    Fix: Let Gradle clear caches unused for 14 days (`mdif tweaks apply gradle-caches`).
 2. Chrome's on-device AI model takes 4.3 GB. Chrome downloads it (about 4 GB) for its built-in
    AI features, and fetches it again if the folder is deleted.
    Fix: Stop Chrome downloading its on-device AI model (`mdif tweaks apply chrome-ai-model`).
 3. ~/Library/Caches/com.example.app was cleared of 6.1 GB 3 days ago and 4.2 GB came back
    since: clearing it by hand doesn't last.
    Leave it: A cache: the app or macOS rebuilds it after it's cleared, so clearing it doesn't
    last. Leave it, unless the app offers a size limit.
```

A tweak is a setting that stops something from filling the disk again: Apple Intelligence off,
Chrome's on-device AI model and its cache size (as a configuration profile you approve in System
Settings), how long Gradle, Homebrew and Claude Code keep what they no longer use, and emptying
the Trash after 30 days. `mdif tweaks apply <id> --dry-run` shows exactly what one changes and
what you give up. Applying one records every value it replaces, so `mdif tweaks revert <id>` puts
them back exactly, except a setting you changed since, which it leaves alone. `mdif tweaks` then
shows whether it worked: the setting still holds, the restart it needed happened, and its folders
shrank or grew slower than before. Commands such as `tmutil deletelocalsnapshots` are only ever
printed for you to run.

Example output from `mdif map` (abridged):

```
Disk disk3  494.4 GB  ·  454.9 GB used  ·  39.5 GB free
  volumes: Data 416.6 GB · System 13.7 GB · VM 11.8 GB · Preboot 11.0 GB · Recovery 1.5 GB
  local Time Machine snapshots: none

Data volume: 416.6 GB in use
  measured        415.6 GB   = 383.2 GB private + 32.5 GB shared with clones/snapshots (counted once)
  not measured      1.0 GB   root-only files, file-system metadata, snapshot-only blocks
  cloud-only      611.4 GB   listed in 34716 placeholders, not on this disk (not counted)
  purgeable        19.1 GB   macOS can delete these by itself when space runs low
```

The whole Data volume is scanned in about 30 s, without root, using `getattrlistbulk`. For
folders protected by macOS privacy controls, give your terminal Full Disk Access.

## Layout

| Crate | What it does |
|---|---|
| `crates/mdif-macos` | Thin, safe wrappers over macOS calls: `getattrlistbulk` (private size, clone IDs, hard links, dataless/sparse/purgeable flags), `F_LOG2PHYS_EXT` (physical extents), FSEvents, `proc_pid_rusage`, `statfs`, `setiopolicy_np`, `sysctl` (boot time, swap) |
| `crates/mdif-core` | Scanner, APFS volumes and snapshots, the reconciled disk map, the watcher, its history store, the forecast, growth per folder, incidents, suggestions and tweaks (reversible settings and configuration profiles) |
| `crates/mdif-cli` | The `mdif` command |
| `app/` | The menu bar app (SwiftUI). It reads everything from the `mdif` in its bundle (`--json`), and `app/check-contract.sh` checks that it can |

## The Mac App Store

The full version can't be there. App Store apps run in the App Sandbox, which blocks reading
other processes' I/O counters (which apps write the most, write churn) and changing other apps'
settings (the tweaks). Releases are Developer ID signed and notarized, and update themselves.

`app/build.sh --app-store` builds an App Store edition into `build/AppStore`: the same app
without the tweaks and the per-app writes, whose code isn't compiled in, and with a note where
they would be that links the full version. It isn't sandboxed or submitted yet; what's left is
in [docs/PLAN.md](docs/PLAN.md) (M7).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Ideas and bug reports are welcome in the issues,
especially "my disk filled up and nothing explained why" stories.

## License

GNU Affero General Public License, version 3 or later ([LICENSE](LICENSE)).
