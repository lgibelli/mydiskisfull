# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## Unreleased

## [0.1.1] - 2026-10-21

### Added
- MyDiskIsFull Lite, the App Store edition, runs in the App Sandbox. Its watcher runs while
  the app does (and the app opens at login if you let it); it sees the disk once you pick it in
  an Open dialog, until then watching free space only; and it keeps its own history.
  `app/app-store.sh` signs it for the Mac App Store, packages and validates it, and uploads it.
- `mdif watch --attached` exits when its standard input closes, so a watcher started by an app
  goes when the app does.
- Finished worktrees: `mdif suggest` and the app find git worktrees whose work is merged into
  the main branch (or squashed into it), with nothing uncommitted and nothing ignored that a
  build doesn't make again, and give their size and the `git worktree remove` command. An
  agent's counts once nothing happened in it for an hour, one made by hand after a week. Git
  only reads: nothing is written into the repository.
- One-click clean-ups: Remove Worktrees for finished worktrees (`git worktree remove`, after you
  confirm; their branches stay) and Move to Trash for Chrome's AI model once its policy is in
  and Chrome has quit. `mdif clean <path>…` does the same from the command line (`--dry-run`
  shows what it would do). Each path is checked again first; anything you keep, or that mdif
  doesn't suggest, is refused. Not in the App Store edition, which may only read the disk.
- The keep list: `mdif keep add <path>` (patterns with `*` and `?`), Keep It on a suggestion in
  the app, and Settings → Kept. Nothing suggests deleting, cleaning or moving what you keep, nor
  a cleanup that would take it with its folder. An incident about a kept folder still shows, with
  "you keep it" as its fix.
- The Overview shows where the space goes: one bar for the disk split by kind (apps, build
  output, developer tools, cloud copies, app data, caches, media, Mail and Messages, virtual
  machines, macOS, your files), each kind biggest first and opening to its biggest items, with
  what may not be needed marked (apps not opened in months, files unchanged for a year, build
  output, caches) and what comes back by itself. A fix shows with what it's about; the rest
  follow under More Fixes. `mdif space` measures it (a whole-disk scan, cached). The Fixes page
  is now Prevent, with the tweaks.
- Fixes show, not tell: a folder that keeps growing has bars of its size at the end of each of
  the last days and now, instead of a sentence; each clean-up says about how much it frees,
  and how old its files are on average (by size, from their modification dates).
- Delete Build Output: a Rust `target` folder, Flutter's `build`, and other build output
  that keeps growing can be deleted from the app (what `cargo clean` does), after you confirm
  and after `mdif clean` checks it again; never something you keep.
- Probably forgotten, at the top of where the space goes: the biggest of what's untouched for a
  year or more, from every kind, each saying what it is. Items say which folder holds most of
  them ("Mostly Outlook › Outlook 15 Profiles: the old Outlook's copy of your mail"), app
  containers go by the app's name ("Microsoft Office (shared)", "Voice Memos").
- What apps keep (`mdif_core::handlers`): Signal's attachments, Messages' attachments, Mail's
  mailboxes, iPhone backups, the old Outlook's mailbox, Teams' web cache, Telegram's media,
  Spotify's cache, Slack, Spotlight's processing, Android emulators and the Photos library are
  recognised in an item, each with what it is, how to make it smaller in the app or in macOS,
  and a button to open the place. It shows with the item when you click its kind on the Space
  bar (App data, Mail and Messages), past the first few if need be; Apps points to App data,
  where what apps keep is. A folder you keep gets no advice to make it smaller.
- Delete node_modules, for a project whose lockfile says what `npm install` puts back; the
  button says it downloads what isn't in npm's cache.
- Empty Trash: the Trash shows apart from what comes back by itself (it doesn't), measured
  fresh each time, with a button in the sidebar that empties it after you confirm.
- Teams' and Slack's web caches get Empty Caches: only what downloads again, while the app is
  closed; messages and sign-in stay.
- Leftovers from macOS updates: the `Relocated Items` folders in `/Users/Shared` (one per update)
  go to the Trash through Finder, which asks for the password once for all of them.
- Cloud storage: iCloud Drive, OneDrive, Google Drive, Dropbox and Box each say how to keep
  fewer files on this Mac and open the app or iCloud settings; a folder left behind when an
  account was removed or reset is flagged.
- Disk Wear, a page of its own: what apps wrote in the last day, against the disk's size, and
  the biggest writers (it was a card on Trends).
- The disk's state moved to the sidebar, on every page: the ring, the forecast in a sentence,
  what's used, the trend, what comes back by itself (with an (i) on why deleting doesn't always
  free space at once) and the Trash. The sidebar can't be hidden.
- Refresh (⌘R and the toolbar) measures everything again; while the window is open free space
  follows what you delete every few seconds, and the breakdown says when the disk changed since
  it was measured.
- Empty Caches asks which, with a checkbox for each. A kind lists up to 20 items (Show More)
  and an Everything Else row, so its rows add up.
- The App Store edition shows what it can't do as locked, with why the App Sandbox rules it out
  and the free full version, and the same clean-up as a command to paste into Terminal.
- Empty Cache: an app's or a tool's cache folder (`~/Library/Caches/<app>`, an app container's
  caches, an Electron app's `Cache`, `~/.cache/<tool>`) is emptied from the Overview, the folder
  itself kept, and Caches has one button for all of them. Not while the app is open (`mdif
  clean` says to quit it first), and never macOS's own caches or one several apps share.
- A helper process that keeps rewriting the disk is named by its app ("Safari keeps rewriting
  the disk", for WebKit's networking process), with Restart Safari's Networking: it ends the
  helper, and Safari starts a fresh one and keeps its windows. `mdif clean <the app>` does it
  only while that helper of that app is the one churning.
- Keep tidy: clean-ups the watcher does once a day, each off until you turn it on (switches
  under Prevent, `mdif tidy on <id>`): simulator runtimes not used in 60 days and the simulators
  left without one (`simctl`), a project's build output (Rust's `target`, Flutter's and Gradle's
  `build`, Swift's `.build`…) that nothing has built into or run from for 14 days, a project's
  `node_modules` nothing has installed into or run from for 14 days (put back from the lockfile),
  a Rust build's
  incremental caches no build has touched for 30 days,
  and app and tool caches of 50 MB or more that nothing has written to or read for 30 days, emptied
  while their app is closed. Every folder goes through `mdif clean`'s checks, never anything you
  keep; what each did last is shown with its switch, and what it deleted is in the watcher's
  log. `mdif tidy run --dry-run` shows what they would do now. Not in the App Store edition.
- The app stays in the Dock like any app; Settings → Show in the Dock takes it out, and then
  it's there only while one of its windows is open.
- MyDiskIsFull Lite has a LITE badge on its icon, in the menu bar, in the sidebar and in the
  menu bar, so the two editions can be told apart when both are installed.
- Cruft: your big files that nothing has opened or changed for a long time, biggest first, on a
  page of their own where you pick how big (50 MB to 10 GB) and how long (6 months to 5 years),
  each with Show in Finder, Keep It and Move to Trash (Put Back returns it). Only files in your
  own folders: not in `~/Library` or `~/Applications`, hidden folders, developer tools' folders,
  packages (apps, Photos and Final Cut libraries, virtual machines) or build output and
  dependencies, and no part of a program (a library, a `.jar`). `mdif space --json` lists them
  in `old_files` (unused for 90 days or more), from the same whole-disk scan, which now reads
  access times too; `mdif clean <file>` checks again that it's still such a file before it goes.
- The Overview is a hub: what needs attention, then the top three from each page (what frees the
  most on Space, the biggest forgotten folders and files on Cruft, what grew, what wrote the
  most, what keeps the disk from filling that isn't on yet), each with its button and See All.
  Where the space goes moved to a Space page of its own (⌘2), with More Fixes; Probably
  Forgotten moved to Cruft, as Forgotten Folders, filtered by the size and age chosen there.
- Trends is now Disk Usage, and shows how full the disk is, in % used, day by day, in layers by
  kind (macOS, snapshots and system data, apps, developer tools, build output, caches, the Trash…)
  from the whole-disk measurements, which go into the history (`strata`; `mdif forecast --series
  --json` gives them); between two the split is estimated, before the first it's faded. Disk Wear
  stacks each day's bar by the biggest writers. Disk Wear has a chart of
  what was written each day, the last 30, against the disk's size, with the day's biggest writers
  under the pointer. `mdif forecast --series --json` gives `writes_daily`: each local day's total
  and its three biggest writers.
- Minecraft's game folder says what it holds (the game, its downloads, the saved worlds by name)
  and whether Minecraft is still installed, and gets Move to Trash, which names the worlds and
  says to copy `saves` first to keep them.
- The Space page's bar is labelled like a railway map: each part has a station on the bar and a
  line in its colour to its name and size, above and below in turn, bending at 45° where labels
  move apart. Clicking a part or its name opens that kind right under the bar (its items, with
  their buttons), the other parts dimmed; clicking it again, or ✕, closes it. The kinds aren't
  listed under it any more: one at a time, the one clicked.
- The sidebar groups the pages: Overview, then Monitor (Disk Usage, Disk Wear, Disk Map) and Clean
  (Space, Cruft, Prevent); ⌘1 to ⌘7 follow it. Refresh and Settings are at the window toolbar's
  trailing edge.
- Settings is a sheet of the main window instead of a window of its own (⌘,, the toolbar's gear,
  the menu bar's Settings…): the first page lists the sections in groups (Watching and
  Notifications; What You Keep and Disk Access), with the app's own row for About & Updates on
  top, a search, and a line under each saying how it's set; a row opens its page, ‹ comes back.
  What's worth a notification and the keep list are on their pages, not in further sheets.
- The menu bar has a plain menu instead of a panel: free space, the forecast and the last day as
  text, what needs attention, Open, Check for Updates, About, Settings and Quit. The Dock icon's
  menu has Hide from Dock.
- After a clean-up the breakdown follows at once: `mdif clean` and `mdif tidy` measure again
  only the folders they changed and patch the cached scan (and its totals), instead of the app
  scanning the whole disk again.
- A cloud folder left behind (`~/Library/CloudStorage/<account> (<date>)`) gets Move to Trash,
  after you confirm; a live account's folder is never touched.
- Notification thresholds you can set (`mdif alerts`, and Settings → Notifications in the app):
  how little free space is an alert (automatic by default: 10 GB or 2% of the disk, or 1 to
  200 GB), how far ahead the forecast warns (full within 14, 3 or 1 days: fewer, later
  warnings), how much a folder must grow to be a runaway (1 GB; 100 MB to 100 GB) and a process
  must write in an hour to be wearing the disk (1 GB; 100 MB to 50 GB), and whether each kind
  notifies. What's detected still shows when its kind doesn't notify. The clauses that keep
  false alarms out (a steady fit, still growing, its share of the folder; writing steadily what
  the disk doesn't keep; a forecast that holds with any day left out) stay as they are. Kept in
  the history; the watcher reads them before each look at the folders, so a change applies
  without a restart. `mdif alerts set <key> <value>`, `mdif alerts reset`; `mdif status
  --json` has them under `alerts`, and its `floor` is the one in effect.

### Fixed
- Cleaning up no longer reads as the disk emptying. After an afternoon of clean-ups (some in
  the app, some in Finder), the forecast read free space growing 23 GB a day: three clean-ups
  in four weeks counted as a pattern, and stayed in. Now steps count as a pattern only when what
  they free fills up again in between (half of it at least), as a cache that's cleared does; a
  burst of cleaning stays a one-off, and on that history the forecast reads 14 GB a day filling.
  A clean-up done through MyDiskIsFull (`mdif clean`, a tidy-up) is also recorded (the last 200,
  for 30 days), and the forecast leaves out every change it falls in, whatever its size, even
  below the step threshold or spread over several samples, and never counts it as a pattern. The
  last hour's pace also leaves out the steps its own 5-minute samples show, so a few deletions
  don't read as the disk emptying fast on a disk whose hours swing by gigabytes.
- A folder that grew and was deleted since is no longer suggested.
- A tweak's switch could stay off after it was applied: a measurement already under way read
  the old state and put it back. The switch now shows the change at once, and a measurement
  asked for meanwhile runs once the one under way is done.
- The app could stop reading the watcher's history: waiting for an `mdif` that had already
  exited could hang for good (`Process.waitUntilExit()` from a task), and every refresh after it
  waited too. The app then said the watcher hadn't sampled for hours, and Start Watching didn't
  help, while the watcher was sampling all along.
- The disk map works where `diskutil` leaves sizes out (in the App Sandbox): the container's
  come from `statfs`, each volume's from the volume itself.

### Changed
- The icon is made in layers, as Icon Composer makes them: macOS 26 and later shape and light it
  like their own instead of showing it small in a grey frame, and earlier versions get it as
  before. `app/build.sh` compiles it with Xcode's actool (Xcode 26 or later), or falls back to
  the flat icon.
- Before a tweak asks you to install a configuration profile, the app says what a profile is
  and shows everything this one holds (one Chrome setting, for your user only) and what it
  doesn't (no certificates, accounts, VPN or remote management), what System Settings and
  Chrome will say, how to undo it, and the file itself. The profile's description in System
  Settings says the same, and `mdif tweaks apply --dry-run` prints it. A tweak waiting for its
  profile has a "Review and Install…" button.
- Profiles are installed in General → Device Management from macOS 15; the app and `mdif` name
  that pane there, and Privacy & Security → Profiles on macOS 14.
- The forecast is a distribution. With a day of history or more, a thousand futures are drawn
  from the last two weeks by blocks of the day (so builds by day and idle nights keep their
  rhythm), recent days counting more, each future with its own mix of days so the range also
  says how sure the rate is. "Full in" is the median of those futures and its range the 10th to
  90th percentile, instead of a straight line at one trend's rate with a range of slopes that
  wasn't a prediction. New in `mdif status --json` and `mdif forecast --json`:
  `forecast.method` ("simulation" or "trend"), `forecast.full_by` (the chance of being full
  within 1, 3 and 14 days), `forecast.paces` (the last hour, 24 hours and 7 days, in bytes a
  day) and `forecast.trend` (the Theil–Sen trend, which is the forecast with less than a day).
  On synthetic histories its 24-hour forecasts are better than the trend's (mean absolute error
  4.2 vs 4.4 GB, and 77% inside the 10–90 range instead of 56%). The app says how likely it
  is ("43% likely within 3 days") and Trends shows what the disk did in the last hour, 24 hours
  and 7 days, as plain amounts, cleanups left out.
- Forecast notifications need the disk more likely than not full in time, with a day and a half
  of history, and still so with any one day of it left out: one heavy day (an afternoon of
  builds, a big download) no longer raises one.
- A cleanup that comes back three times in four weeks (a weekly one) is part of the forecast;
  before, only three within two weeks counted. Gaps in the samples are spread over their hours
  when they're up to two days long (a sleeping Mac consumes nothing) and unknown when longer.

### Security
- `mdif clean` and the app's buttons refuse a folder or file reached through a link on the way
  (`~/devel/link/target` with `link` pointing elsewhere), as they already refused a link itself.

## [0.1.0] - 2026-10-20

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

- The app (`app/`, SwiftUI, built with `app/build.sh`). A main window with a sidebar and four
  pages, ⌘1 to ⌘4: Overview (a capacity ring and the forecast in a sentence, led by whatever
  would fill the disk soonest; what needs attention, each with its fix; the last days; the
  biggest fixes), Trends (free space with the forecast and its plausible range, the alert floor,
  cleanups, readings under the pointer; what grew and wrote most), Disk Map (a storage bar, then
  a sunburst, a treemap or a list, coloured by folder or, like a market heatmap, by each
  folder's growth in the last day; click to zoom in) and Fixes (suggestions, and the tweaks as
  switches that apply or undo after saying what they change and what you give up). The map
  keeps itself current while it's open: measured again on opening if the last map is over 30
  minutes old and every hour while it stays open, at background priority, the previous map
  showing meanwhile. Never in the background, though: one whole-disk map takes about 200 s of
  CPU, 200 times the watcher's budget for an hour. In the menu bar, a glance: free space, the
  forecast, the last three days and what needs attention, with the way into the window. The app
  joins the Dock only while a window is open, and stays in the menu bar when it starts at login.
  Settings, a first-launch welcome and an About panel with the watcher's measured cost.
  Notifications for a new incident, the forecast crossing 14, 3 and 1 days or free space under
  10 GB, and a tweak something undid, once per episode. The watcher runs from the bundle as an
  SMAppService LaunchAgent (its own label, `…mydiskisfull.watcher`) and takes over from
  `mdif agent install`.
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

[0.1.1]: https://github.com/lgibelli/mydiskisfull/releases/tag/v0.1.1
[0.1.0]: https://github.com/lgibelli/mydiskisfull/releases/tag/v0.1.0
