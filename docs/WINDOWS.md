# MyDiskIsFull on Windows

The same watcher, forecast and explanations, for NTFS. This is the plan and where it stands.
`docs/PLAN.md` is the product; this file is only what Windows changes.

## How it is built

Everything the operating system does sits behind `mdif_core::os`, which re-exports the one
backend of the target and pins each function's signature, so the two can't drift:

| | macOS | Windows |
|---|---|---|
| Crate | `crates/mdif-macos` | `crates/mdif-windows` (on `windows-sys`) |
| Listing a folder | `getattrlistbulk` | `FileIdExtdDirectoryInfo` on an open folder handle |
| Change journal | FSEvents, replayed from a saved ID | the NTFS change journal, read without administrator rights, replayed from a saved position |
| Per-process writes | `proc_pid_rusage` | `GetProcessIoCounters` (read, not recorded: pipes count as writes) |
| Cloud placeholders | `setiopolicy_np`, `SF_DATALESS` | `FILE_FLAG_OPEN_NO_RECALL`, the recall attributes |
| Volumes | `statfs`, APFS containers | `GetDiskFreeSpaceExW`, volume GUIDs |

`mdif_core::portable` holds what `std` only gives on Unix (allocated bytes, file identity, link
count, owner, file modes). The rest of `mdif-core` is one copy of the code on both systems.

The CLI is `mdif` / `mdif.exe` with the same commands and JSON. The Windows app
(`app-windows/`, Tauri) runs it the way the Mac app does, and `app/Tests` holds the contract: the
window's logic is tested against those files (`node --test app-windows/ui/tests/*.test.mjs`).

## Where it stands

Phase 1 (done): the workspace builds, lints and tests on x86_64 Windows (built and run on a
runner, and on a Windows 11 PC), is type-checked for aarch64 Windows (its tests aren't run there;
the ARM64 installer and its programs run on a Windows on ARM runner), and the Mac is unchanged. What runs on Windows today is the free-space forecast: `mdif watch`, `status`,
`forecast`, `alerts` (without write churn's), `keep`, `knowledge` (Ask AI included), `owners`, `incidents` (growth per folder from the NTFS journal), `map`
(NTFS drives, saying what it can't be exact about), `space` (the Windows drive sorted into kinds:
see below), `clean` (a project's build output, its packages and finished worktrees, and a
drive's Recycle Bin emptied, `C:\$Recycle.Bin`; the rest is refused by name), `suggest` (what the watcher found, finished worktrees and build output, with
PowerShell's commands; snapshots, swap and Docker's disk image are a Mac's), `tweaks` (see below)
and `agent` (the watcher at sign-in,
through the user's `Run` key and `mdif-watch.exe`, the same program without a console window;
`agent stop` and `agent start` are Windows' own, for the installer). The rest says it isn't available on
this operating system yet, rather than printing numbers built on another system's picture of a
disk: `docker`, `timemachine` and `writers`.

Automatic clean-ups on Windows (`mdif tidy`, the window's Prevention page): old build folders
(.NET's `bin` and `obj` beside a project file among them), old `node_modules` and old Rust build
caches; no simulators or Time Machine, and no caches or Docker leftovers until `clean` empties them
there. The first two go by what was last written *and read* in them, as on a Mac, so they're offered
only where NTFS records when a file was last opened (`fs::records_last_access`), which since Windows
10 1803 is off on a system drive over 128 GB; elsewhere they're shown greyed, with the reason. An
administrator can turn that recording on (published guides give `fsutil behavior set
disablelastaccess 0` and a restart; one reports the values the other way round, so check with
`fsutil behavior query disablelastaccess` first; not tried on a PC). Rust's incremental caches go by
the date changed alone: every build writes them. The code folders are those in the home folder by
name, whatever the case (NTFS has one folder for "Code" and "code"), and Visual Studio's
`source\repos`.

Tweaks on Windows (`mdif tweaks`, the window's Prevention page): Gradle's caches and Claude Code's
transcripts (the same files as on a Mac), and "Empty the Recycle Bin automatically after 30 days",
which is Storage Sense's, set in the user's registry (`Setting::Registry`,
`HKCU\Software\Microsoft\Windows\CurrentVersion\StorageSense\Parameters\StoragePolicy`): "01"
on, "08" on, "256" 30 days unless a shorter wait is in effect, "2048" weekly unless it already runs
daily or weekly. The value names aren't documented by Microsoft; they're as Settings writes them
and as published lists have them (`tweaks::STORAGE_POLICY`). MyDiskIsFull never turns Storage
Sense on where that would start a clean-up of the user's own files (Downloads, or OneDrive's files
made online-only: as set in Settings; by Group Policy where it sets the days, 0 being never; or by
Windows' default since 11 22H2, after 30 days, wherever OneDrive syncs an account, told by OneDrive's
`UserFolder` under `HKCU\Software\Microsoft\OneDrive\Accounts`. There an account's own Never doesn't
count, as which of Storage Sense's keys is whose isn't known: only Group Policy's 0 lets it through),
nor runs it weekly where it runs less often and such a clean-up is on, nor where Group Policy decides
(`storage_sense_refusal`). Undo
puts back every value it replaced, as on a Mac. Windows reads it at the next sign-in. Unit tests
use a scratch key; CI applies the real tweak on its runner and puts it back. On a PC that syncs
OneDrive and never set Storage Sense (no `StoragePolicy` key, no policy) it's refused, as Windows'
default would make OneDrive's files online-only. Nobody has yet checked on a PC that Storage Sense
then empties the bin, nor what such a PC has under those keys once Storage Sense is set.

| Phase | What | State |
|---|---|---|
| 0 | Spikes on the runner: the NTFS journal without admin, listing speed, whether network I/O counts as writes (placeholders and toasts still to measure) | measured, below |
| 1 | The seam: builds and tests on Windows, no change on the Mac | done |
| 2 | The backend for real: listing from a handle (allocated size, file number, links), process list and writes, Recycle Bin, moved folders by number | listing and process list done; emptying the Recycle Bin done; moving to it, moved folders open |
| 3 | The watcher and trends: the change journal, the agent (a `Run` key), owner rules for Windows, growth per folder and incidents, the e2e test on an NTFS volume (a VHDX mounted in a folder), the budget job | done, on a real runner; per-process writes wait for evidence from the disk |
| 4 | Windows' own catalogue: what fills a disk there, handlers, clean-ups, tidy-ups, tweaks (registry), the backups page | |
| 5 | An optional elevated service for the journal at full precision and the file table | |
| 6 | The app (Tauri) | started: tray, notifications, opening at sign-in; Overview, Storage, History, Settings |
| 7 | Packaging (a per-user installer, signing, updates) | started: the installer builds and is tested on the runner; signing and updates open |

### The app and its installer

`app-windows/src-tauri` is the app (its own Cargo workspace, Rust 1.90 for Tauri 2.12):
a tray icon whose tooltip says what's free and what fills it, a window that hides to the tray when
closed, one instance at a time, and `--background` at sign-in. It never measures anything: it runs
the `mdif.exe` beside it (`mdif.rs`: a fixed list of commands, a time limit, no console window).
Every 10 minutes, and when the window comes forward, it reads `mdif status --json` and notifies as
the Mac app does: each runaway once per episode, the alert level when it goes up (down only after
six hours), what it said kept in `announced.json`. Its first window is the Mac's welcome, less the
two steps Windows doesn't ask for (reading the disk, notifying): start the watcher, then Get
Started, from which it opens at sign-in (a per-user `Run` value written by `mdif-windows`'
`startup`, so Task Manager's switch is respected); Settings turns that off. What's about programs'
writes (the write-churn alert, History's Writes) is left out, as none are recorded on Windows.
The app also does what launchd does for the Mac's watcher: one that has ended though it's set to
start at sign-in is started again with `mdif agent start` (`monitor.rs`, `Supervisor`), once it's
been seen ended at two looks two minutes apart (an update stops it a moment before it closes the
app; a slow sign-in starts it late, and a second watcher refuses to run beside the first), at most
once an hour, and never when Task Manager has it switched off or it was stopped on purpose:
`mdif agent stop` says so in `agent status --json`'s `stopped` until it's wanted again (a watcher
starts, or `agent start` or `agent install` runs: those also call off a stop that gave up waiting,
so the watcher running then stays).
`app-windows/ui` is the window: plain HTML, CSS and ES modules, no packages. The RailBar is drawn in SVG with the Mac's layout (checked against the Swift code on 200
random bars); a clean-up goes through `mdif clean`, the question before what can't be undone is a
dialog, and what it did is a banner. Clean Up (Overview's toolbar, and beside Recommendations) is
the Mac's sheet: everything the app can clean in one list, what comes back by itself ticked, what
needs a look (the Recycle Bin) unticked with what can't be undone beside it, run one after another,
each row saying what it did (`cleanup.js`, the list from `text.js`' `cleanUpRows`). Storage's
Folders view is the Mac's disk map (`folders.js`, the layouts in `map.js`): a sunburst, a squarified
treemap coloured by folder or by the last day's growth, and an outline, from `mdif map --depth 6
--min-mb 200`; measured only while the view is on screen (when it opens and the kept map is over
half an hour old, hourly while it stays, at idle priority when nobody asked) and kept in the app's
cache folder (`map.rs`) so the last one shows at once. A window hidden in the tray or minimised
measures nothing by itself (it asks the app, `window_shown`), and neither does the one loaded at
sign-in until it's first shown; whole-disk measurements (the map, the categories) run one after the
other, one at a time in the app too, and one that failed isn't retried by itself for an hour. The
caption carries `mdif map`'s notes on what it can't measure exactly. Opened in a browser, the window reads the files in
`app/Tests` (or `?fixtures=<folder>`) instead of running anything, for working on it anywhere.

`app-windows/build.sh [target]` builds `mdif.exe` and `mdif-watch.exe`, copies them to
`src-tauri/binaries/<name>-<target>.exe` and builds the NSIS installer with `tauri.release.json`,
which adds them to it (kept out of `tauri.conf.json`, as `tauri-build` wants them on every build).
The installer is per user (`%LOCALAPPDATA%\MyDiskIsFull`, no administrator), and
`src-tauri/windows/hooks.nsh` handles the watcher, which runs from that folder: Windows lets no one
replace a running program's file, so an update runs `mdif agent stop` first and `mdif agent start`
after (which starts it as sign-in would: not if Task Manager has it switched off), and an
uninstall runs `mdif agent uninstall`. The history shares the install folder; the uninstaller
removes only its own files, so the history stays unless "Delete the application data" is ticked.
CI's `windows-app` job builds the installer, installs it silently, starts the app (it reads the
disk, and sets nothing up before its welcome is done), installs the watcher, updates over it twice (once with the watcher switched off in Task Manager), ends the watcher
and sees the app start it again, uninstalls,
checks the registry, processes and files at each step, and keeps the installer as an artifact.

## What the runner measured (Phase 0)

`gh workflow run ci.yml --ref <branch> -f spikes=true` runs `crates/mdif-windows/tests/spikes.rs`
on a `windows-latest` runner (Windows Server, an administrator account). What it said, and what
each answer decides:

| Question | Measured | So |
|---|---|---|
| Do network and pipe traffic count as a process's writes (`GetProcessIoCounters`)? | A 64 MiB file: 64 MiB written. The same over loopback TCP: 0 written, 63 MiB under "other". The same down a pipe to a child: 64 MiB written (65,536 operations). | Sockets are safe. Pipes (a build streaming into a tool, a shell pipeline) count as writes, so the churn detector can't trust the counter alone: it already wants the process's folders not to grow, and on Windows it should also want the change journal to show activity under them. |
| Listing speed | `FileIdExtdDirectoryInfo` on an open handle: 255k entries/s, the same as `std::fs` (259k/s) over the same 4,000 folders. | List from a handle: allocation size and file number come free. |
| Is a file open for writing shown at its size? | No. A 4 MiB file held open for writing is listed as 0 by `std` and by the handle listing, and as 4 MiB by a stat of the file itself; the listing then lags one write behind. | A growing log is the thing the product hunts, so recently changed files are stat'ed by handle after the listing (13 µs each, bounded to what changed lately). |
| Link counts | Open for attributes plus `GetFileInformationByHandle`: 13 µs per file (11 µs is the open). | The listing has no link count. Ask only for files big enough to matter (say 1 MiB and up), counted once by file number. A smaller file is taken to have one name, so one with another name outside the scanned tree is counted as freed by deleting it when it isn't: the error is bounded by that size per file, and the report has to say it isn't exact. |
| NTFS journal without administrator rights | With a restricted token (an administrator's filtered token, the closest a runner gets to a standard user): the volume device opens with `FILE_TRAVERSE`, and `FSCTL_QUERY_USN_JOURNAL` and `FSCTL_READ_UNPRIVILEGED_USN_JOURNAL` work, on the volume device and on a handle to `C:\` opened with no access at all. `FSCTL_READ_USN_JOURNAL` is denied (error 5). | Reading the journal needs no service and no UAC. That a change is found again by replaying from a saved position was shown afterwards (Phase 3: `changes::tests::a_change_is_found_again_from_a_saved_position`, run on the runner, and the end-to-end test on a volume with its own journal); the probe itself only read from the first retained record. The elevated service (Phase 5) is then only for full precision. The restricted token is an approximation: it still needs checking as a real standard account. |

Not measured yet: cloud placeholders (needs a sync root), toasts from an unpackaged program, the
`StartupApproved` behaviour of a `Run` key, WebView2.

## Known gaps, to carry into the next phases

- The listing is read from one open, checked folder handle (`FileIdExtdDirectoryInfo`): allocated
  size, file number and reparse tag come with it, and every record is bounds-checked before it is
  read. Still open:
  - The link count isn't in the listing, so only files of 1 MiB or more are asked for it, by
    opening them by path (`dir.join(name)`), which looks the name up again. Opening the child
    relative to the checked folder handle (`NtCreateFile` with a root directory) closes that gap.
  - A count that can't be had (a file held open without sharing, as the page file) is
    `LINKS_UNKNOWN`: the scanner counts it once but never as freed by deleting the folder, and
    `mdif map` shows it apart ("names unknown"); the owner tracker splits it by two.
  - A file under 1 MiB is taken to have one name, so its other names are counted again: on a
    system drive the component store's hard links come to a few GB more than the drive has in
    use, and `mdif map` says the scan counted that much twice rather than showing a negative
    remainder.
  - The owner tracker's re-listings (`Detail::Sizes`) ask the 1,024 most recently changed files of
    each folder for their own size and link count, by path like the link counts above: a log held
    open is listed behind otherwise.
  - A file with several names shows the size of the name that was listed; another name's entry
    can be stale until it's closed. A handle's own size would be current.
  - The file number is the low 8 bytes of the 128-bit ID. NTFS only uses that much; on ReFS two
    siblings can share it, so ReFS volumes have to keep the whole ID before they are scanned.
  `mdif map` and the owner tracker read folders this way.
- `mdif map` measures NTFS drives only: FAT, exFAT and UDF lack the listing's class, and ReFS
  keeps 128-bit file numbers. Still open:
  - A volume mounted in a folder (`C:\mnt\data`) is a mount point to the listing, which doesn't
    enter it, so `--root` at that folder lists nothing of it; measure it by a drive letter.
  - WOF-compressed files (CompactOS, `compact /exe`) count what NTFS allocates for them, about
    nothing: their data is in a stream the listing doesn't size.
  - A child folder is opened again by its path, so a name Win32 rewrites (a trailing dot or
    space, a device name such as `COM3`) fails to open, or opens another folder.
  - A `SUBST` drive has the figures of the volume under it, so the whole-drive comparison is off.
  - ReFS is told by the root's own path: a root reached through a junction to another volume is
    checked as the volume the junction is on.
- The owner tracker on Windows:
  - A file directly in a folder above where owners are (the profile folder, a drive's root,
    `AppData\Local`) is an owner of its own from 1 MiB (`C:\pagefile.sys`, `~\huge.log`), at most
    the 1,024 biggest per folder (one that comes in among them counts at least what it grew past
    the smallest followed, one that drops out what it changed by); smaller ones aren't counted
    anywhere.
  - A folder seen for the first time credits only the files made since the last period, unless
    it was made since then itself (then all it holds). So a file moved into an old folder the
    watcher hasn't listed yet (the Recycle Bin's, the first deletion after the watcher starts)
    counts as freed where it was and not as added where it went, until the bin is emptied.
  - A folder that moves and changes in the same period takes what it held along: folders seen
    for the first time are listed after the known ones (and after new folders holding known
    ones), so its old place is found gone, and followed, before its new place is measured. Unless
    its old place is deferred that period (a big busy folder past the period's budget): then it
    still counts as freed there and new where it went.
  - A file held open and growing (silent in the journal until it's closed) is kept in view
    across a watcher restart: the folders being followed (at most 256) are kept with the
    journal's checkpoint and listed again at the start. What it grew while no watcher ran isn't
    known: the first listing after the restart is a first sight.
  - WSL's and Docker's disk images (`ext4.vhdx`, in folders named with a hash) aren't on the
    growth allowlist, as virtual machines' folders are (nor is Docker's disk on a Mac).
  - A rescan the journal asks for that would list more than 20,000 folders stops there and is
    counted as a gap, so incidents say some changes weren't seen.
- `own_processes` lists the user's processes (token's SID compared with ours) with their I/O
  counters, but a Windows write counter counts pipes and sockets, so the watcher records no
  writers (`WRITES_ARE_DISK_WRITES` is false) and `writers` stays refused, until churn has
  evidence from the disk (changes under the process's folders, or the service's ETW disk I/O).
- History is keyed by the volume's identity (`FsStats::history_key`: its GUID), and the watcher
  re-checks it on every sample, so a disk swapped under a drive letter starts its own history.
- The journal's position is a `u64`: the USN in eights (44 bits, 140 TB) under a 19-bit tag of the
  journal's id, so a journal made again is a gap (one in half a million is mistaken for the same).
  It is read with the unprivileged call on the root of the volume the watched folder is mounted
  on; a volume with no journal, or none this user may read, is a gap, and so is a journal cut
  while it is replayed (what was found before stays). A folder this account can't open (another
  user's, a service's) is left out of the replay, not counted as a gap. It hasn't been checked as
  a standard (non-administrator) account, only as an administrator, and ReFS (128-bit file
  numbers, version 3 records) isn't read. Owner tracking is on: the watcher replays the journal every 30 minutes by default, and CI
  grows a folder under a running watcher and finds it with `mdif owners`.
- When a replay fails with an error, write-churn detection is skipped with growth detection (it
  shares the evaluation); on Windows there are no recorded writers anyway. A journal that can't be
  opened at all is recorded as a period with a gap.
- If the disk under a running watcher is swapped, folder tracking stops (the free-space history
  carries on under the new disk's own key), but an incident that was open stays open until the
  watcher is restarted. The Windows advice for a growing folder is text only and doesn't check
  the keep list for kept files inside a temp folder.
- The watcher at sign-in is `mdif-watch.exe`, the CLI source linked for the GUI subsystem by
  `build.rs` (MSVC only: Windows GNU builds are not supported, and would show a console). One
  watcher per history is a lock file beside it, held across sign-ins, and `agent uninstall` asks
  it to finish by making a `.stop` file it looks for each second (no process is ended from
  outside); the `Run` command line is
  limited to 259 characters, so `agent install` refuses a longer path. `agent install` starts the
  watcher in its own folder and with none of its handles (a program reading what `mdif` says sees
  it end), and calls off a stop request an uninstall left pending.
- A mapped network drive has no volume GUID, so its history is keyed by its drive letter and a
  letter remapped to another share would continue the first one's. Watch local disks.
- `mdif space` sorts the Windows drive by its own rules (`space/windows.rs`): Windows (with
  the page, hibernation and swap files as items), Program Files and the Store's apps,
  ProgramData, the profile's AppData (Temp, Packages, browsers' and Electron apps' caches), WSL
  distributions and VM folders, OneDrive and other cloud folders, Outlook's files, build output
  in code folders (`source\repos` too), the Recycle Bin, and what only Windows can read as
  System data. The only clean-up offered on an item is deleting build output and a project's
  packages (`node_modules` with a lockfile), which `mdif clean` checks again as on the Mac. The
  app offers to empty the Recycle Bin (asked first, as the Mac app asks before emptying the
  Trash): `mdif clean C:\$Recycle.Bin` empties this user's bin on that drive with the shell's
  `SHEmptyRecycleBinW`, after `SHQueryRecycleBinW` says there's something in it, and the report
  measures the bin as it is now, so emptying it shows at once. Moving to the bin (`fs::trash`,
  `shell.rs`) is the shell's `IFileOperation` on a COM thread of its own, with a progress sink
  that refuses an item the shell marks to be deleted for good: left as it is, never deleted. The
  shell doesn't mark everything it deletes for good: a file bigger than the bin takes is marked
  to be recycled and then deleted for good (seen on Windows 11, in another project's CI and on a
  real PC), so `fs::trash` refuses before the shell is asked a drive or a share as a whole, a
  volume with no bin (removable, network, a share, one with no letter or mounted in a folder), a
  bin set to delete at once (`NukeOnDelete` under `BitBucket\Volume\{GUID}`, or the policy
  `NoRecycleFiles`), and a file bigger than the bin takes (`MaxCapacity` there, in MiB, or the
  policy `RecycleBinSize`, a percentage of the quota; with neither, Windows' default, a tenth of
  the first 40 GB of the user's quota and a twentieth of the rest). A folder too big for the bin
  goes item by item, each marked, which the sink refuses. CI moves a file and a folder to the
  runner's bin, checks a share's file is refused, and with the bin set to take 4 MB for a moment
  that a file of 8 MB stays (and that one of 1 MB goes), then with it set to delete at once that
  nothing goes. On a new volume of 50 GB (a VHDX with a letter: past 40 GB, so both parts of the
  default count) whose bin was never set, it checks the default: what Windows writes for that bin
  takes at least as much, the biggest file the default lets through is recycled, and one a tenth
  bigger is refused and stays. **Old Files** uses it: the report's `old_files` are big files of yours
  (`clean::yours`, with `space::windows::own_file` on Windows: in the profile, in no folder
  sorted as apps' data, apps, a cloud folder (deleting there deletes online: OneDrive's,
  Dropbox's, iCloud's, Creative Cloud's by how their names begin, and the plain-file services'
  by name, Google Drive's `My Drive` among them), a virtual machine, a developer tool, caches,
  mail or build output; not a VM's disk, a `.pst`, a driver, nor an `.exe` outside Downloads; not
  hidden or a system file), and `mdif clean` moves one to the bin after `unused_file` checks it
  again, which also refuses a file a sync service manages wherever it is (a cloud placeholder's
  attributes, downloaded or not: `portable::managed`) and a path through a short name below the
  home folder (`APPDAT~1`, which the by-name checks and the keep list don't know). Most NTFS drives
  don't record when a file was last opened (`NtfsDisableLastAccessUpdate`; since 1803 the
  system leaves it off on a drive over 128 GB): `fs::records_last_access` reads that (NTFS
  only), the report says `last_opened_unknown`, and "old" is then what nothing changed (the
  later of the two times, as on a Mac), which the page says. Apps have no "last opened" (there
  is no Spotlight), and a disk quota's figures aren't reconciled into System data. .NET's `bin` and `obj` (any case:
  `owners::dotnet_output`) are build output beside a project file (`*.csproj`, `*.fsproj`,
  `*.vbproj`): sorted so in code folders (Visual Studio's `source\repos` too), deleted by
  `mdif clean`, and owners of their own growth; without one, a `bin` is a folder of the project's
  like any other (people keep scripts there), so they aren't among the names the rest takes as
  build output (`owners::ARTIFACTS`). Only on Windows: on a Mac nothing suggests them, and
  `mdif clean` refuses them. A folder whose note sends to Settings → System → Storage (Temp,
  Windows Update's downloads, `Windows.old`) has a button that opens it (`ms-settings:storagesense`).
  What apps keep in the profile is told by `handlers::WINDOWS` (looked up on Windows only, names
  matched whatever their case): OneDrive, iCloud Drive and Dropbox (Free up space, Make
  online-only), iPhone backups in either place, Spotify, Slack, Telegram and the new Teams. They
  only say the way: nothing in them is emptied, moved or opened from the app yet.
- The Time Machine and knowledge code knows only the Mac's folders (`~/Library`): the backups
  page and Windows entries in the knowledge list are Phase 4.
- Ask AI: the ChatGPT sign-in's token file is made the user's alone with an access-control list
  (`mdif-windows/src/private.rs`: one entry, the user's, inherited from nothing, the user its
  owner), and checked on every read: one that took its folder's list, or was copied in, isn't
  used, and sign-out says to delete it and revoke the sign-in. What's sent names Windows' version
  (`sys::windows_version`: "11 24H2", from the registry) under the key `windows`, the AI is told
  it's a PC's folder (with Windows examples), and only generic names go
  (`knowledge::generic_on_windows`: AppData and an app's folder in it, a tool's dot-folder,
  Windows' own places on a drive; never your folders, another account's, a share or a drive's
  other folders). CI runs the sign-in, ask, renewal and sign-out against the stand-in OpenAI on the
  runner (curl.exe from System32); nobody has signed in for real from a PC. The knowledge list
  itself still has no Windows entries (Phase 4).
- The app and its installer:
  - An update from a build without `mdif agent stop` can't replace a running `mdif-watch.exe`
    (NSIS asks to retry). Nothing released lacks it.
  - The app's own children are left running if it's ended from outside, so an update that ends
    the app during a long `mdif space` can't replace `mdif.exe` until that finishes.
  - The installer and the programs aren't signed, so SmartScreen warns; there is no updater yet
    (`tauri-plugin-updater` is in the cache; it needs a signing key of the author's). CI builds
    both installers on x64 and runs each where it belongs: `windows-app` the x64 one,
    `windows-app-arm` the ARM64 one on a Windows on ARM runner (`windows-11-arm`), with the same
    script (`app-windows/tests/install.py`): each installs, updates over a running watcher and
    uninstalls on its runner. The ARM64 one (`app-windows/build.sh aarch64-pc-windows-msvc`,
    also cross-built from a Mac: its three programs are ARM64, the installer itself x86 as
    NSIS's always is) has run on ARM hardware only there.
  - Pages not ported yet: Backups. Prevention is (the automatic clean-ups as switches, with what
    each did last and Tidy Now; its tweaks as switches, each applied after a look at what it
    changes, undone by putting back what it replaced). Ask AI is (Settings →
    What Folders Are: the switch, Continue with ChatGPT, Sign Out, Manage usage; in Storage, an
    item nothing's known about has Ask AI…, which shows what would be sent before it's sent, and
    an answer says it was written by AI and not checked). Old Files is
    (`pages/oldfiles.js`: big files of yours and folders unchanged for as long as asked, ticked
    ones kept or moved to the Recycle Bin together, no question asked: the bin gives them back).
- Docker isn't supported: its data folder, for the keep rule, and its liveness check come with
  the catalogue.
- `GetDiskFreeSpaceExW` is on the calling user's basis, so with a disk quota "free" is what the
  quota leaves.

## Checking it from a Mac

No Windows machine is needed to type-check, and the runner tests it. Once: `rustup target add
x86_64-pc-windows-msvc aarch64-pc-windows-msvc`, and for `--offline` the target's dependencies
(`windows-sys`) must be in the cargo cache.

```sh
LIBSQLITE3_SYS_USE_PKG_CONFIG=1 cargo clippy --offline --workspace --all-targets \
  --target x86_64-pc-windows-msvc -- -D warnings
```

(`LIBSQLITE3_SYS_USE_PKG_CONFIG` stops SQLite's C source from being compiled for a target there is
no C compiler for. A check doesn't link.) `.github/workflows/ci.yml` has a `windows` job that
lints, tests, builds and smoke-tests on a real runner, a `windows-arm` job for aarch64, a
`windows-app` job for the app and its installer, and the MSRV job type-checks Windows on Rust 1.88.

The app type-checks the same way from `app-windows/src-tauri`, with LLVM's `llvm-rc` on the `PATH`
for its resources (`brew install llvm`):

```sh
cd app-windows/src-tauri
PATH="$(brew --prefix llvm)/bin:$PATH" cargo clippy --offline --all-targets \
  --target x86_64-pc-windows-msvc -- -D warnings
```

With `cargo-xwin` (which downloads Microsoft's CRT and SDK once) and `makensis`,
`app-windows/build.sh` builds the whole installer from a Mac, though it can't be run there.
