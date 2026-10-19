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
| still growing (the last third of the window grew at least a quarter of the fitted rate) and within 2% of its peak | anything that has plateaued or started shrinking |
| at the fitted rate, fills the remaining free space within the horizon (default 7 days) | slow, harmless growth on a roomy disk |
| owner not on the allowlist (path components) | Photos imports, VMs, deliberate downloads |

**Churn** (something writes far more than it keeps):
- Per-process bytes written come from `proc_pid_rusage`, without root.
- A process is flagged when it writes more than 1 GB/hour for over an hour while its owned
  folders grow by less than 10% of that, for example a log rotator gone wild or a stuck sync.
- This catches what size-only monitoring cannot see. It also wears out SSDs.

**Regrowth memory:** after any cleanup, the app records how fast the item comes back. "Cleared
6 GB on Oct 16, back to 4 GB by Oct 17: not worth clearing; [suggestion]."

### 3. Suggestions: how to avoid it (done in M3)
Each cause maps to a fix that stops it from coming back (`mdif suggest`, and a "Fix:" line under
every incident). A fix is a **tweak** when a setting can stop it, otherwise a command to run once,
steps to take in the app, or the advice to leave it alone because it refills by itself.

Tweaks (`mdif tweaks`) are reversible. Applying one records every value it replaces before it
changes anything, and a failure halfway undoes what was done. Reverting restores only the settings
that still hold the tweak's value, so a change made since is kept. Each tweak is checked later:
the settings still hold, the restart it needed happened, and its folders shrank or grew slower
than in the week before (from the watcher's history).

| Cause | Suggestion | How the app applies it | Admin? |
|---|---|---|---|
| Apple Intelligence models | turn Apple Intelligence off | tweak: every numeric opt-in flag in `com.apple.CloudSubscriptionFeatures.optIn` (the number differs between macOS versions) and `auto_opt_in` set to false; takes effect after a restart | no |
| Chrome on-device AI model (4 GB) | stop it downloading | tweak: Chrome policy `GenAILocalFoundationalModelSettings = 1` in a user-scoped profile; once it's in effect, remove the copy | approve profile |
| Browser cache growth | cap the cache | tweak: Chrome policy `DiskCacheSize` (500 MB) in a profile | approve profile |
| Build output regrowth (`target/`, `node_modules`, `build/`) | clean it with its own tool; share one target dir between worktrees; auto-clean stale builds (M5); exclude from Time Machine | the command (`cargo clean --manifest-path …`, `flutter clean`), `build.target-dir` advice; rules in M5 | no |
| Agent worktrees piling up | remove merged, clean worktrees | `git worktree remove` advice; automatic rule in M5 | no |
| Homebrew, Gradle caches | retention settings | tweaks: `HOMEBREW_CLEANUP_MAX_AGE_DAYS=30` in `~/.homebrew/brew.env`; a Gradle init script (14 days, from Gradle 8.0). Cargo cleans its own cache since 1.88. | no |
| AI coding tool transcripts | retention | tweak: Claude Code `cleanupPeriodDays = 14` | no |
| Trash | empty it automatically | tweak: Finder's `FXRemoveOldTrashItems` (30 days) | no |
| Docker disk image | prune it; cap it in Docker's settings | `docker system prune`. Shrinking the image deletes its contents, so no tweak. | no |
| Local Time Machine snapshots holding deleted space | release them (when the disk is tight) | `tmutil deletelocalsnapshots <date>` | no |
| Swap | restart | shown with its size when the disk is tight | no |
| Cloud files kept downloaded | free up space / unpin | Finder's Free Up Space (steps); OneDrive needs its own `MarkUnpinned` action (research) | no |
| Stuck sync client (churn) | restart it | quit and relaunch (known apps only: an unknown process name never goes into a command), then the churn clause shows whether writes dropped | no |
| Chat backups (WhatsApp), Messages "keep for", Photos/iCloud "Optimize" | change the setting | no API, so the steps in the app | — |
| A cache that refills after a cleanup | leave it | regrowth memory says so | — |
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
    mdif-cli/     `mdif map | watch | forecast | owners | incidents | suggest | tweaks | writers`
                  (later `clean | tm`)
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
- **M2 – Owners + incidents (done).** Owner sizes kept incrementally from FSEvents journal
  replay plus the open files of heavy writers, growth and churn clauses, one-sentence
  explanations, episodes, regrowth memory, and the e2e test extended to all of it. Learned on
  the way, and handled:
  - FSEvents reports a file's folder when the file is created or closed, not while it's
    written, so the heavy writers are asked which files they have open (`proc_pidfdinfo`).
  - fseventsd keeps no journal for volumes mounted `-nobrowse`; only live events exist there.
  - Listing costs about 3.5 µs per entry whatever is asked for (private size doubles it), so
    folders with thousands of entries are re-listed at most every 2 hours.
  - A folder seen for the first time has no previous size: only files created since the last
    period count, so the first period is a lower bound. A folder moved as a whole is not seen.
  - `diskimages-helper` rewrites everything written into a mounted disk image, so it is
    allowlisted for churn (its clients are measured); attributing its writes to the image in
    the writers list is still to do.
- **M3 – Suggestions + tweaks (done).** Cause → fix catalogue (`mdif suggest`, a fix under every
  incident), seven tweaks with apply/verify/revert recorded in the history, configuration
  profiles for Chrome's policies, and `diskimages-helper` writes shown with the image they go
  to. Learned on the way, and handled:
  - macOS no longer installs profiles from the command line. The app writes a user-scoped
    `.mobileconfig` (the Custom Settings payload Apple Configurator writes) and opens it; the user
    approves it in System Settings. It is in effect once its keys appear in
    `/Library/Managed Preferences/<user>/`, and removing our own needs no password.
  - Apple Intelligence's foundation models (`AssetsV2/…UAF_FM_*`) can't even be listed without
    root, so the tweak's effect shows as free space, not as a folder shrinking.
  - Gradle's `removeUnusedEntriesAfterDays` is write-only; the init script was checked offline
    against Gradle 8.11 and 8.14 by reading the property behind it (30 → 14 days).
  - Homebrew's `brew.env` takes `HOMEBREW_*` lines verbatim: no `export`, and quotes would be
    part of the value.
  - `defaults delete <domain>` leaves an empty plist behind.
  - Docker's disk image is sparse (64 GB long, 5 GB allocated here), and shrinking its cap deletes
    its contents, so Docker gets advice, not a tweak.
  - macOS 27 deprecates `hdiutil create/attach` in favour of `diskutil image`; the e2e test
    still uses `hdiutil`.
  - The first real incident on the development Mac was churn by `cfprefsd`: 1.1 GB an hour that
    the disk didn't keep. cfprefsd saves preferences for other apps; here a chat app was saving a
    2.8 MB preferences file (thousands of logging-policy keys) every 4–11 seconds. Churn by
    cfprefsd is now blamed on the app whose big preferences file was just rewritten.
  - Measured cost of the watcher at its defaults on that Mac (five agent sessions building):
    samples take about 5 ms of CPU and 33–44 KB of writes. The first owners period after a start
    took 1.9 s (3,956 folders seen for the first time), the next 0.3 s (1,566 folders). Steady
    state is about 0.65 s of CPU and 0.4 MB of writes an hour: inside the budget, but much
    closer to it than on a CI runner (6 KB a sample). Averaged over the 76-minute run, the
    catch-up made it read 1.7 s an hour, over budget, so the cost now measures samples and
    folder updates apart and shows the catch-up on its own. Replaying FSEvents costs almost nothing
    (2 ms for 1,000 events); listing is the cost, and six folders of 6k–40k entries (a browser
    cache, Rust `deps`) were 80% of one listing.
- **M4 – Menu bar app.** Gauge, incidents with their fixes, tweaks with one-click apply and undo,
  map and clean screens, notifications (including a tweak that was undone), onboarding for Full
  Disk Access.
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
- The budget is measured, not estimated: self CPU and writes per sample and per folder update
  (the first update after a start reported apart), checked in CI on a 2-minute run and scaled to
  the defaults (12 samples and 2 updates an hour).

## Open questions
- UI shell: Tauri, or a native Swift menu bar like Idlewild driving the Rust core?
- Notifications from the agent need an app bundle; ship the agent as a helper inside the app?
