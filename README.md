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

> **Status: early development.** The honest disk map (`mdif map`) and the disk-writer monitor
> (`mdif writers`) work today. The watcher, forecasting, incidents and the menu bar app are next;
> see the [plan](docs/PLAN.md).

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
  each clone family once, and keeps cloud-only placeholders separate. The total reconciles with
  what APFS reports in use.
- **Never costs you network traffic.** Scanning turns off cloud-placeholder downloads for the
  process, so looking at an iCloud or OneDrive folder never downloads it.
- **Prove it before deleting.** Only regenerable data, or data with a verified copy, is offered
  for deletion. Personal files only ever go through a verified move to your cloud or an archive.

## Try it

Requires macOS and Rust 1.88+.

```sh
git clone https://github.com/lgibelli/mydiskisfull.git
cd mydiskisfull
cargo build --release
./target/release/mdif map              # where the space is, reconciled with APFS
./target/release/mdif map --root ~     # just your home folder
./target/release/mdif writers          # which processes are writing to disk right now
```

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
| `crates/mdif-macos` | Thin, safe wrappers over macOS calls: `getattrlistbulk` (private size, clone IDs, dataless/sparse/purgeable flags), `proc_pid_rusage`, `statfs`, `setiopolicy_np` |
| `crates/mdif-core` | Scanner, APFS volumes and snapshots, the reconciled disk map |
| `crates/mdif-cli` | The `mdif` command |

## Not on the Mac App Store

It can't be. The App Sandbox blocks reading other processes' I/O counters and other apps'
containers, which is most of what this tool needs. Releases will be Developer ID signed and
notarized.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Ideas and bug reports are welcome in the issues,
especially "my disk filled up and nothing explained why" stories.

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
