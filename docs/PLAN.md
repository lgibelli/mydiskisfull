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
  app/            menu bar app: native Swift (SwiftUI MenuBarExtra), built by app/build.sh;
                  bundles mdif, and registers `mdif watch` as its LaunchAgent (SMAppService)
```

- **The agent owns all the watching**, so the UI costs nothing while closed. The menu bar shows
  "days to full" next to its icon once the forecast is within two weeks.
- **The app never measures anything itself.** It runs the `mdif` inside its bundle with `--json`
  (`status` every 10 minutes and when the menu opens; `suggest`, `tweaks` and `map` on demand),
  so the command line and the app can't disagree. `app/check-contract.sh` decodes the real output
  with the app's models in CI, and fixtures cover what a fresh history can't show.
- **Native Swift, not Tauri.** The menu, notifications with actions, login items and a
  future widget are native APIs either way, a web view would render untrusted file and process
  names next to commands that change settings, and the Idlewild build and release pipeline
  (swiftc without an Xcode project, Developer ID, notarization, Sparkle) is reused as is.
- **History:** SQLite, decimated, size-capped.
- **Distribution:** Developer ID signed, notarized, stapled, with signed updates (Sparkle; see
  M6 and `docs/RELEASING.md`). Not the Mac App
  Store: its sandbox blocks `proc_pid_rusage` on other processes and reading other apps'
  containers. A reduced App Store edition is possible (M7): `app/build.sh --app-store` compiles
  the app with `APP_STORE`, which leaves out the tweaks and the per-app writes and shows a note
  linking the full version where they would be (`app/Sources/Edition.swift`).
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
- **M4 – Menu bar app (in progress).** Built so far, in native Swift like Idlewild:
  - The menu: free space and the forecast, open incidents with their fix, tweaks something
    undid, the watcher's state, "Fix It for Good" (`mdif suggest`) and the tweaks, each a
    checkmark that applies or undoes it after showing what it changes and what you give up.
  - Windows: an overview (gauge, free-space chart with the forecast and cleanups, what grew and
    wrote most), the disk map (sunburst, treemap or list, click to zoom; the treemap is coloured
    like a market heatmap by each folder's growth in the last 24 hours from the watcher's
    history), settings, a welcome for the first launch, and About with the watcher's measured
    cost.
  - Notifications: a new incident (with "Apply Fix…" or "Copy Command"), the forecast crossing
    14 days, 3 days and 1 day or free space under 10 GB (2% on big disks), and a tweak that was
    undone; each once per episode, withdrawn when it's over. The forecast levels need the 3-day
    trend, so an afternoon of builds can't raise one.
  - The watcher runs from the bundle as an SMAppService LaunchAgent and takes over from
    `mdif agent install` (same history); `mdif agent install` won't start a second one.
  - `mdif status --json` (everything the menu needs, read from the history only), `mdif forecast
    --series`, `--json` for `tweaks apply` and `revert`, `mdif watch --log`.
  - Learned on the way, and handled:
    - **A background process must never touch a folder that shows a privacy prompt.** Without
      Full Disk Access, opening Desktop, Documents, Downloads, iCloud Drive, cloud storage or
      another app's container shows a dialog and `open` waits for the answer: the agent stopped
      sampling for 18 minutes on the development Mac. The watcher checks for Full Disk Access at
      every sample, and until it has it leaves those folders (and other volumes) alone
      (`privacy`), and records the check for the app to show. The same goes for `mdif status`,
      which the app runs unattended. The map and suggestions run when asked, so they may prompt.
    - From the command line the watcher's TCC identity is its own binary (`~/.cargo/bin/mdif`),
      and an adhoc signature changes with every build, so grants don't survive rebuilds. Run by
      the app, its checks are made as the app: one grant covers both. The app is signed with a
      Developer ID from `release.env` (gitignored) when there is one.
    - The disk map is kept (`~/Library/Caches/<bundle id>/disk-map.json`) with when it was
      taken. It's measured again while someone looks: when the window opens and the kept one is
      over 30 minutes old, and every hour while it stays open, at background priority, the old
      one showing meanwhile. Never in the background: one whole-volume map measured about 200 s
      of CPU (191 s of it in the kernel) over 104 s at background priority on the development
      Mac, 200 times the watcher's budget for an hour.
    - The app's watcher needs a label of its own. Background Task Management keeps a record of
      a command-line LaunchAgent under its label, disabled once it's removed, and an
      SMAppService agent with the same label inherits it: "Job is not allowed to bootstrap",
      reported to the app as "Operation not permitted".
    - macOS 27 has no per-user privacy database (`~/Library/Application Support/com.apple.TCC`
      is gone), so Full Disk Access is checked by opening the system's, which also needs it.
    - Notifications turned off for the app in System Settings can't be asked for again
      (`requestAuthorization` fails with "not allowed"); the app says so and opens the pane.
    - Foundation's `.convertFromSnakeCase` turns `writes_24h` into `writes24H`; the app converts
      keys itself.
    - The app doesn't have the shell's environment, so where a tweak lives
      (`GRADLE_USER_HOME`, `XDG_CONFIG_HOME`) can differ from where the command line put it.
      Applied tweaks are judged by the settings their record names. Applying from the app still
      uses the app's environment; to do: read the login shell's.
    - **The watcher was over budget on a busy Mac.** On the development Mac (several agents
      building) the 30-minute folder updates cost 1.5–1.7 s of CPU each, about 3.5 s an hour
      with the samples, against 1 s. A profile puts nearly all of it in `getattrlistbulk`, at
      10–15 µs an entry from a background process rather than the 3.5 µs measured in the
      foreground. Replaying 55 minutes of the journal: 2,400 folders changed, 228,000
      entries, 64% of them in five folders of 8,000+ entries (two builds' `deps`, Chrome's and
      Slack's disk caches, `~/.wrangler/logs`), 5% in the 2,300 folders under 64 entries.
      Tuned (owners.rs): a folder whose size changed under a fifth of the slowest growth an
      incident can have (16 MB a period at the defaults) since its last listing is steady and
      waits 2×, then 4× its size-based wait (8 hours at most): caches and logs churn but keep
      their size, while a filler keeps growing; and an update lists at most 20,000 entries of
      folders it has seen before, the most overdue first. (A fixed 16 MB made the e2e test's
      scaled-down filler look steady; listed in lumps, its incident flickered closed and open
      again.) On that journal the updates go from about 2 s an hour to 0.5 s (all
      steady) or 0.8 s (the builds growing), and the cap holds them under 0.4 s whatever the
      Mac does. Growth still counts in full, later; a restart loses what a waiting folder grew
      since its last listing (little, by definition).
  - Still to do: the clean screen (with M5's rules), notifications delivered on a signed build,
    and confirming the tuned watcher's cost over a day on a busy Mac.
- **M5 – Cold data + offload, Clean, TM, cloud health.** Cold-folder detection, offload
  strategies over the verified Move flow, rules ported from the original shell script, TM
  monitor, OneDrive database. The privileged helper (SMAppService daemon) for the clean-ups
  that need admin rights (Spotlight, root-owned apps, `/Library` caches) comes with the clean
  screen, its only user: moved here from M6, since a root daemon with nothing to do would add
  an approval prompt and attack surface to every install.
- **M6 – Release (done, 0.1.0).** Notarization, signed updates, the download page.
  - `app/release.sh`: a universal build (`ARCH=universal app/build.sh`), the app notarized and
    stapled, then a disk image signed, notarized and stapled in its own right (Gatekeeper
    assesses the image too), checked by mounting it and assessing the app inside as a download
    is; the command line alone as a tarball (the binary from the bundle, so the same
    notarization ticket); the Sparkle feed, signed. Every step checks its result rather than
    trusting an exit status (Idlewild's lesson: `codesign | grep -q` under pipefail fails when
    it matches). `--upload` makes a *draft* release; a person publishes it.
  - Signed updates with Sparkle 2.10, vendored from the pinned release tarball and re-signed
    with the team's identity, as in Idlewild. The feed is the appcast on the latest GitHub
    release (`releases/latest/download/appcast.xml`), so there's no server: publishing the
    release ships the update. Trust is the Ed25519 key (its own keychain account, not shared
    with other apps) plus the team signature. A menu bar app can't take the focus, so Sparkle's
    gentle reminders put it in the Dock and post a notification while an update waits. After
    an update the app restarts the watcher (`launchctl kickstart -k`), which would otherwise
    keep running the replaced `mdif` until the next login.
  - `.github/workflows/release.yml`: the same from a `release/x.y.z` branch, with a provenance
    attestation; needs the secrets listed in `docs/RELEASING.md`. It replaces the tag-triggered
    workflow that published only the command line.
  - The download page is the GitHub release and the README's Install section; a site of its
    own isn't needed while the feed lives on the release.
- **M7 – App Store edition.** Done: the build flag, the tweaks and per-app writes compiled
  out, the notes, a CI check that the tweak flow isn't in that binary. To do:
  - The App Sandbox (entitlements for the app and for `mdif`, which inherits it), and
    checking on a scratch Mac or CI runner what really works inside it.
  - The watcher: a sandboxed SMAppService agent gets a container of its own, apart from the
    app's, so the two need an app group for the history; or the app watches while it runs
    and starts at login. Either way nothing may run after the app quits without consent
    (App Review 2.4.5).
  - Access: the user picks the disk in an Open dialog, kept as a security-scoped bookmark, in
    place of Full Disk Access; the welcome and the access cards change with it.
  - `mdif` without per-process write sampling and the tweak commands (a cargo feature).
  - Packaging: Apple Distribution certificate, an App Store provisioning profile, an
    installer package uploaded with Transporter.
  - Review risk: a link to download a version from outside the store can be rejected. If
    it is, the note can say what's missing without the link.

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
- None open for M4's design. (Answered: the SMAppService watcher's privacy checks are made as the
  app, `subject=io.github.lgibelli.mydiskisfull` in tccd's log, so one Full Disk Access grant
  covers both.)
