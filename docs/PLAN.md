# MyDiskIsFull: plan

A macOS app that watches your disk over time, predicts when it will be full, catches whatever is
filling it, says *why* in one sentence, and tells you how to stop it from happening again.
Free and open source, with no telemetry and no account.

Disk analyzers show the current picture once you go and look. MyDiskIsFull watches all the time
and speaks up when something is filling the disk fast enough to matter.

## Why

The project began with a real cleanup session on a 494 GB Mac that had 6 GB free. Most of what
filled it was invisible to snapshot tools, because the problem was *growth*, not size:

- A cloud sync client got stuck. It uploaded 0 bytes while rewriting a rolling 3 GB of logs at
  about 5 MB/s (roughly 18 GB/hour of SSD writes). Its folder size barely moved, so only write
  activity gave it away.
- An analysis cache was cleared, and macOS rebuilt 4 GB of it overnight. Clearing it was
  pointless, and history would have shown that.
- A messaging app added a 3.7 GB backup in one day. Spotlight re-indexed 2.3 GB. Rust builds grew
  1.6 GB/day.
- 45 git worktrees created by coding agents held 36 GB, mostly a Flutter `build/` of 2–5 GB each.
- A local Time Machine snapshot kept 26 GB of deleted files allocated. Nothing came back until it
  was released.
- `du` double-counted APFS clones (a cloud client's Finder view and its internal folder share
  blocks), so even the "current picture" was wrong.

Before the first byte of this code, the session ended at a projected "full in 19 days" with
nothing to warn about it.

## The design constraints (borrowed from Idlewild)

1. **A monitor must never become the thing it hunts.** Budget: under 1 s of CPU per hour, under
   1 MB/hour of its own disk writes, and history capped at 20 MB. The About panel shows its own
   consumption, measured.
2. **False positives are the whole product.** Coding agents writing build output in a burst,
   Xcode, Docker, Photos imports and backups are all legitimately big writers. Every incident
   must pass strict clauses (below), and allowlist entries match whole path components, never
   substrings.
3. **Explain why, in one sentence, with the evidence.** Example: "OneDrive is rewriting 3 GB of
   logs every 10 minutes while uploading nothing; usually a stuck sync. Restarting OneDrive fixes
   it."
4. **Then say how to avoid it.** Every incident ends in an action that stops the regrowth (a
   tweak, a setting, a cleanup rule), not just "delete this once".
5. **Honest numbers.** APFS private size, not `du`. Every byte on the volume belongs to a row,
   and the total matches what APFS reports in use.
6. **Prove it before deleting.** An item must be regenerable or have a verified copy (sync
   database, git pushed). Personal data is never deleted automatically.
7. **Network-aware.** Never download or upload as a side effect. Scanning disables cloud
   placeholder downloads for the process (`setiopolicy_np`). Actions that cost traffic are
   labelled and held on metered connections.

## Features, in order of importance

### 1. Forecast: when will the disk be full?
- Free space on every volume, sampled every few minutes from `statfs` (free, and essentially
  zero cost). History is decimated: 5-minute slices for a day, hourly for a month, daily for a
  year.
- The forecast is a robust trend over several windows (6 h, 3 days, 14 days). Cleanups and purges
  are detected as step changes and excluded, so freeing 20 GB doesn't read as "the disk is
  shrinking". It reports "Full in about N days at the current rate", with a range, and says which
  window it uses.
- It's explained by owners: "of the 3.1 GB/day, 1.6 GB is one project's Rust build, 0.9 GB is
  WhatsApp backups, 0.6 GB is everything else".
- Warnings come at escalating horizons (14 days, 3 days, 24 h), plus an alert when free space
  drops below a floor. Like Idlewild, each episode alerts once.

### 2. Runaway fillers
Two kinds, each with its own clauses.

**Growth** (something keeps getting bigger) is an incident only when *all* of these hold:

| Clause | What it removes |
|---|---|
| a fitted line over the sustain window (default 6 h, at least 8 points), positive slope, r² ≥ 0.85 | bursts, staircases (copied a file, stopped), sawtooth caches |
| grown by at least 1 GB *and* at least 5% of its own size | noise, which is proportional to size |
| latest sample within 2% of its peak | anything that has plateaued |
| at the fitted rate, fills the remaining free space within the horizon (default 7 days) | slow, harmless growth on a roomy disk |
| owner not on the allowlist (path components) | Photos imports, VMs, deliberate downloads |

**Churn** (something writes far more than it keeps):
- Per-process bytes written come from `proc_pid_rusage`, without root.
- A process is flagged when it writes more than 1 GB/hour for over an hour while its owned
  folders grow by less than 10% of that, for example a log rotator gone wild or a stuck sync.
- This catches what size-only monitoring cannot see. It also wears out SSDs.

**Regrowth memory:** after any cleanup, the app records how fast the item comes back. "Cleared
6 GB on Oct 16, back to 4 GB by Oct 17: not worth clearing; [suggestion]."

### 3. Suggestions: how to avoid it
Each cause maps to a fix that stops it from coming back. Fixes are reversible: the app records the
old value, checks later that the fix worked (using the trend data), and can undo it in one click.

| Cause | Suggestion | How the app applies it | Admin? |
|---|---|---|---|
| Apple Intelligence models (≈8 GB) | turn Apple Intelligence off | user pref `com.apple.CloudSubscriptionFeatures.optIn` (+ `auto_opt_in`), or a profile that blocks the features | no / approve profile |
| Chrome on-device AI model (4 GB) | stop it downloading | Chrome policy `GenAILocalFoundationalModelSettings = 1` (profile) | approve profile |
| Browser cache growth | cap the cache | policy `DiskCacheSize` | approve profile |
| Build output regrowth (`target/`, `node_modules`, `build/`) | auto-clean stale builds; share one target dir between worktrees; exclude from Time Machine | cleanup rule + `CARGO_TARGET_DIR` advice + sticky TM exclusion | no |
| Agent worktrees piling up | remove merged, clean worktrees automatically | `git worktree remove` rule | no |
| Homebrew, Gradle, Cargo caches | retention settings | `HOMEBREW_CLEANUP_*`, Gradle `caches {}` init script, cargo `[gc.auto]` | no |
| AI coding tool transcripts | retention | Claude Code `cleanupPeriodDays` | no |
| Docker disk image | cap its size | Docker Desktop `diskSizeMiB` | no |
| Local Time Machine snapshots holding deleted space | release them | `tmutil deletelocalsnapshots` | no |
| Cloud files kept downloaded | free up space / unpin | system eviction; OneDrive needs its own `MarkUnpinned` action (research) | no |
| Stuck sync client (churn) | restart it | quit and relaunch, then confirm writes drop | no |
| Chat backups (WhatsApp), Messages "keep for", Photos/iCloud "Optimize" | change the setting | no API, so a deep link to the right settings pane with instructions | — |
| Spotlight, root-owned apps, `/Library` caches | cleanup with admin rights | privileged helper (SMAppService) | yes |

### 4. Cold data: what you never open, and offloading it
Large files and folders you haven't used in months are the cheapest space to win back. They
don't have to be deleted, only moved somewhere you can still reach.

**Finding them.**
- **Signals:** access time (`ATTR_CMN_ACCTIME`; APFS updates it, verified on the Data volume),
  modification time, date added (`ATTR_CMN_ADDEDTIME`) and Spotlight's last-opened date (often
  empty, so it only adds to atime).
- **The scanner reads metadata only.** Reading contents would reset atime and destroy the signal.
- **Cold is per folder, not just per file.** A film collection nobody opened for a year, a VM not
  booted for 6 months, a project with no commit, build or edit for 6 months.
- **Ranked by space won × confidence:** "48 GB of videos in Desktop/Storage, none opened since
  2025-03".
- **Excluded:** files that are cold by nature (app bundles, system assets, the Photos library
  internals, mail stores).

**Offload strategies**, ordered by how well they keep files reachable:

| Strategy | When | How it stays safe |
|---|---|---|
| Into your cloud provider (OneDrive, iCloud Drive, Google Drive, Dropbox), then free up space | you have one, with quota left | Move flow: clone-copy, hash every file, wait until the provider confirms each file is synced, then evict. The files stay visible as placeholders and open on demand. |
| iCloud "Optimize Mac Storage" / Desktop & Documents | iCloud user | guided setting |
| Git project archive: delete a local clone that is fully pushed and clean | old repos | checks: no unpushed commits, stashes, untracked files or local-only branches; re-clone with one click |
| Compress, then offload (VMs, datasets, project folders) | large, compressible, rarely needed | archive integrity verified before the original is removed |
| External drive / NAS archive | no cloud or no quota | hash-verified copy; reminds you the archive needs its own backup |

**Before moving anything:**
- Check the provider's quota.
- Estimate upload time, and hold uploads on metered connections or hotspots.
- Show the effect on the forecast: "offloading these 48 GB adds about 120 days before the disk is
  full".

### 5. Map: the honest current picture (done in M0)
- Volumes, snapshots and purgeable space, plus a tree measured with APFS private size, clone
  families counted once, and cloud-only placeholders counted separately.
- It reconciles with APFS: on the development Mac, 415.6 GB measured vs 416.6 GB reported in use
  (0.24% unexplained), the whole Data volume in 27 s with 44 MB of memory and no root.

### 6. Clean: recommendations
Rules each have a detector, safety checks, an action and a regrow profile: build output (with
busy detection that tells language servers apart from builds), merged agent worktrees, package
caches, duplicate toolchains (several Flutter SDKs, Homebrew rust vs rustup), Xcode simulator
runtimes, leftovers of deleted apps, known space hogs, Downloads installers, and AI-tool scratch
folders in `/private/tmp`. Personal data only goes through the **Move flow**: clone-copy, hash
every file, wait for the cloud provider's database to confirm each file is synced, then delete the
original.

### 7. Time Machine monitor
- Backup status, destination usage vs quota, and how big each backup is (TM logs).
- Which owners went into each backup (FSEvents plus exclusion state), e.g. "node_modules added
  3 GB to your backups this week".
- Exclusion health: regenerable folders are excluded automatically with sticky xattrs, re-applied
  when FSEvents sees a folder recreated. `tmutil addexclusion` takes about 11 s per path, so it
  runs in parallel.
- Local snapshots that are holding space. Note: TM exclusions don't shrink local snapshots,
  because APFS snapshots cover the whole volume.

**Incident: Time Machine is failing and its backlog is eating the disk.**

*What happens.* When backups keep failing (destination unreachable, full, or erroring), Time
Machine keeps taking hourly local snapshots it can't send anywhere. Each one pins every block that
changed or was deleted since the last good backup. Free space then drains with no file to blame:
deleting things frees nothing, and `du` sees nothing.

*How it's measured:*
- **Backlog age:** time since the last *successful* backup, plus the reason for each failure from
  `log show --predicate 'subsystem == "com.apple.TimeMachine"'` (destination unreachable, disk
  full, quota, auth, an interrupted run).
- **Space the snapshots hold:** the map's reconciliation measures it. Volume in use minus what the
  scan can attribute (clones counted once) is space only snapshots and metadata keep. Without
  snapshots it is about 1 GB here; with them it rises, which gives a "held by snapshots" size and
  growth rate.
- **Snapshot count and age** from `tmutil listlocalsnapshots`.

*Incident clauses:* no successful backup for longer than the expected interval × 6 (default 24 h)
*and* snapshot-held space growing *and* that growth materially shortens the forecast.

*Example sentence:* "Time Machine hasn't completed a backup since Tuesday (the NAS is unreachable).
6 local snapshots now hold 31 GB, growing 8 GB/day; at this rate the disk is full on Saturday."

*Suggestions:*
1. Fix the destination (reconnect, check quota or credentials) and run a backup now.
2. Release the snapshots (`tmutil deletelocalsnapshots`, no admin needed), and warn that
   unbacked-up changes then have no local snapshot either.
3. If the failures are chronic, exclude the high-churn folders that make each backlog hour
   expensive.

### 8. Cloud sync health
- Detects providers in `~/Library/CloudStorage` and iCloud.
- OneDrive: reads a read-only copy of `SyncEngineDatabase.db`, which gives per-file sync state
  (`fileStatus` 2 = synced and downloaded, 8 = cloud-only; `eTag`). It also tracks queue state,
  runaway logs, and the size of the client's internal copy.

## Architecture

```
mydiskisfull/
  crates/
    mdif-macos/   FFI: getattrlistbulk (private size, clone ID, dataless/sparse/purgeable flags),
                  proc_pid_rusage, statfs, setiopolicy_np, FSEvents, NSFileManager eviction
    mdif-core/    scanner, owners, map; history store + decimation; forecast and incident
                  detection; rules, suggestions, tweaks; providers; TM monitor
    mdif-cli/     `mdif map | watch | forecast | writers | clean | tm | tweaks`
    mdif-agent/   the watcher: tiny headless LaunchAgent, within the budget above
  app/            menu bar app (Tauri 2, opens a window only on demand); notifications
```

- **The agent owns all the watching**, so the UI costs nothing while closed. The menu bar shows a
  small gauge with "days to full".
- **History:** SQLite, decimated, size-capped.
- **Distribution:** Developer ID signed, notarized, stapled, with signed updates. Not the Mac App
  Store: its sandbox blocks `proc_pid_rusage` on other processes and reading other apps'
  containers.
- **Builds offline** from the local cargo cache. Everything the macOS layer needs is in
  `libc` 0.2.190.

## Milestones
- **M0 – Core + map (done).** macOS FFI, honest map reconciled with APFS, `mdif writers`, CI.
- **M1 – Watch engine (done).** History store, free-space sampling, forecast with step-change rejection,
  per-process write sampling, and `mdif watch` / `mdif forecast`. Plus an e2e test that runs a real
  filler (a file growing at a fixed rate) and a real churner (rewriting a file in place), and
  asserts that both are detected and nothing else is.
- **M2 – Owners + incidents.** FSEvents incremental owner sizes, growth and churn clauses,
  one-sentence explanations, regrowth memory. Writes relayed by `diskimages-helper` (it rewrites
  everything written into a mounted disk image to the image file) are attributed to the image,
  not counted a second time: the e2e test shows them as the largest writer.
- **M3 – Suggestions + tweaks.** Cause → fix catalogue, apply/verify/revert, profiles.
- **M4 – Menu bar app.** Gauge, incidents, map and clean screens, notifications, onboarding for
  Full Disk Access.
- **M5 – Cold data + offload, Clean, TM, cloud health.** Cold-folder detection, offload
  strategies over the verified Move flow, rules ported from the original shell script, TM
  monitor, OneDrive database.
- **M6 – Release.** Privileged helper, notarization, signed updates, website.

## Verification
- Unit tests on synthetic fixtures: clones, sparse files, unreadable folders, symlinks, worktrees
  with symlinked `node_modules`.
- Forecast tests on recorded and synthetic series: steady growth, sawtooth, cleanup steps,
  plateau.
- e2e: a real filler and churner, as Idlewild does with a real CPU burner and leak.
- The budget is measured, not estimated: self CPU and self writes per sample, checked in CI on
  a 2-minute run and scaled to the default 5-minute interval.

## Open questions
- UI shell: Tauri, or a native Swift menu bar like Idlewild driving the Rust core?
- Notifications from the agent need an app bundle; ship the agent as a helper inside the app?
