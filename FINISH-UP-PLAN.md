# Finish-up plan: the Windows port

Branch `windows-port`, CI green on every job at f90fb5f (macOS, real Windows runner, Windows on
ARM, MSRV 1.88); the app, its installer and the `windows-app` job came after and haven't run on a
runner yet. The long-form plan and findings are in `docs/WINDOWS.md`; this file is what is left.

## Where it stands

Decisions (from the start): Tauri app, parity with the Mac before a first Windows release, a GitHub
Actions Windows runner as the only test box, a per-user watcher with an optional elevated service.

| Phase | What | State |
|---|---|---|
| 0 | Spikes on the runner | done, findings in `docs/WINDOWS.md` |
| 1 | The seam (`mdif-windows`, `mdif_core::os`, `portable`, gating) | done |
| 2 | The backend: listing from an open handle, process list, `path_of`, volumes with no letter | mostly done (see "Open in Phase 2") |
| 3 | Watcher and trends: NTFS journal, `agent`, owner tracking, `owners`/`incidents`, e2e on a VHDX, budget job | done, on a real runner |
| 4 | Windows catalogue | **started**: `mdif map` runs; the rest is open |
| 5 | Optional elevated service | not started |
| 6 | Tauri app | **started**: tray, notifications, sign-in; Overview, Storage, History, Settings |
| 7 | Packaging, signing, updater | **started**: the NSIS installer and its hooks, tested by CI; signing and updater open |

What runs on Windows today: `watch`, `status`, `forecast`, `alerts`, `keep`, `knowledge` (Ask AI too),
`agent` (with `stop` and `start`, Windows' own, for the installer), `owners`, `incidents`, `map`, `space` (its only action: Delete for build output and
packages), `clean` (build output, `node_modules` with a lockfile, a Rust build's incremental caches,
finished worktrees; the rest refused by name), `suggest` (the history's incidents, growth and
cleanups that came back, finished worktrees, build output, the tweaks; not yet snapshots, swap or
Docker), `tweaks` (Storage Sense's Recycle Bin clean-up, Gradle, Claude Code), `tidy` (old build
folders, `node_modules` and Rust's incremental caches; the first two only where Windows records when
a file was last opened). Still refused with "isn't available on this operating system
yet" (`NOT_HERE_YET` in `crates/mdif-cli/src/main.rs`): `docker`, `timemachine`, `writers`. When a command's Windows half lands: remove its name from
that list, remove its `#[command(hide = !cfg!(target_os = "macos"))]` (the commands are hidden from
`--help` on Windows until then), and remove it from the refusal loop in `ci.yml`.

## `mdif map` on Windows

Done, with the findings of its review: estimates are said in words (`notes` in the JSON, `note:` in
the text: small hard links, files held open for writing, a disk quota), a ReFS root is refused, a quota
account gets no reconciliation, a partly downloaded cloud file counts what is resident, and the
reconciliation is shown for any drive's root. Run on a real Windows 11 PC too (C:, about 30 s): a file
whose names can't be counted (the page file) is shown apart, and the component store's small hard
links, which make the scan come to more than the drive has in use, are said as counted twice rather
than as a negative remainder. FAT, exFAT and UDF drives are refused like ReFS. Still open: hard-link
counts for files under 1 MiB and a fresh size for files held open (both are noted, not measured), a
snapshot/restore-point line, and the gaps listed in `docs/WINDOWS.md` (a volume mounted in a folder,
WOF-compressed files, names Win32 rewrites, `SUBST` drives).

## Phase 4: the Windows catalogue (parity with the Mac)

Each item goes through its own engine and its own tests with temp dirs (and a scratch registry key),
never the user's real settings.

- **`space`**: **done, read-only** (`space/windows.rs`; a CI step on the runner's C: drive, and a
  whole-report test that runs on Windows). System files (WinSxS, Installer, SoftwareDistribution,
  `pagefile.sys`, `hiberfil.sys` and `swapfile.sys` as items), leftovers (`Windows.old`,
  `$Recycle.Bin`), Program Files, the Store's apps and ProgramData, the profile (AppData
  Local/Roaming, Packages, Temp, browsers' caches), dev folders, WSL and Docker, VMs, cloud
  folders, Outlook. What can't be read goes to "System data". Still open: its actions (they come
  with `clean`), app "last opened" dates (UserAssist, to verify first), names for Store packages (`Microsoft.WindowsTerminal_8wekyb3d8bbwe`).
  .NET's `bin`/`obj` are build output beside a project file (see `clean`). Not yet run on a real
  Windows PC.
- **`handlers`**: **started**: `handlers::WINDOWS`, looked up on Windows only and matched whatever
  the case: OneDrive, iCloud Drive and Dropbox, iPhone backups (both places), Spotify, Slack,
  Telegram, the new Teams; their way said, nothing emptied, moved or opened yet. Still open: mail
  (OST/PST explained and never deleted), game libraries (outside the profile: handlers start from
  the home folder), browsers.
- **`clean` and `tidy`**: **`clean` started**: build output, packages, incremental caches and
  finished worktrees are deleted (`clean::plan` refuses the rest on Windows with
  `NOT_ON_WINDOWS`; Windows-only tests in `clean::windows_tests` and a CI step on the runner).
  **Emptying the Recycle Bin done**: `mdif clean C:\$Recycle.Bin` (exactly a drive's bin: nothing
  inside it, no folder of its name elsewhere) calls `fs::empty_recycle_bin` (`SHEmptyRecycleBinW`
  for that drive, no UI, after `SHQueryRecycleBinW` counts something in it); the app's Overview and
  Storage offer it with the Mac's question; the space report measures the bin as it is now; CI
  empties the runner's bin for real. **Moving to the Recycle Bin written** (`fs::trash`,
  `mdif-windows/src/shell.rs`): `IFileOperation` with a progress sink that refuses what the shell
  marks to be deleted for good; refused first, as the shell marks a file too big for the bin to
  be recycled and then deletes it for good: a drive with no bin, a bin set to delete at once, and
  a file bigger than the bin takes (its settings and Group Policy's, or Windows' default); CI's
  `windows` job runs it for real, the too-big case with the bin set to 4 MB for a moment (not run
  yet: needs a push). Old Files moves big unused files of yours with it
(`clean::unused_file`); handlers will too. **.NET's `bin`/`obj`
  done**: build output beside a `*.csproj`/`*.fsproj`/`*.vbproj` (`clean::dotnet_project`, the
  folder's names only, any case), checked again when the report sorts them (`space::classify`),
  owners of their own growth, and Visual Studio's `source\repos` a projects folder; Windows only
  (`owners::dotnet_output`: refused on a Mac, where nothing suggests them); tests on both systems
  and in CI's clean step. Still open: emptying a cache while
  its app is closed (checked by process image path, using
  `proc::terminate_program`'s identity check), Docker (`docker.exe`), Android. Add `temp`
  (`%TEMP%` older than 7 days *and* not in use, not on the keep list, and re-checked by `clean::plan`:
  age alone is not enough, a program may still need what it wrote last week). Every new kind goes through `clean::plan` and `clean::run`, which
  re-check each path. Simulators and backups drop out. **`tidy` written** (`tidy::Id::ALL` is
  per system): old build folders (.NET's `bin`/`obj` beside a project file too, and Visual
  Studio's `source\repos` is a code folder), old `node_modules` and old Rust build caches, run by
  the watcher once a day as on a Mac; the window's Prevention page has them as switches with Tidy
  Now. Builds and `node_modules` are offered only where NTFS records when a file was last opened
  (`fs::records_last_access`): on most PCs it doesn't (since Windows 10 1803 the default leaves it
  off on a system drive over 128 GB: `fsutil behavior query disablelastaccess`), and by the date
  changed alone a tool still run from `target\release`, or the packages of a project that runs,
  look old; there they're greyed with that reason. Incremental caches go by the date changed
  (every build writes them). That's a decision the author may revisit (the date changed alone,
  said so). Caches and Docker's leftovers wait for `clean`'s Windows half of them. Tests: on every
  system (`tidy::portable_tests`) and a CI step on the runner (not run yet: needs a push).
- **`tweaks`**: written. `Setting::Registry` (numbers under `HKCU`, through
  `mdif-windows/src/registry.rs`, which `startup` uses too); the Windows catalogue is the Recycle
  Bin's clean-up by Storage Sense plus Gradle and Claude Code. Still open: whether Storage Sense
  then empties the bin, seen on a PC; more tweaks (Chrome/Edge model policies need
  `HKCU\Software\Policies`, which a user can't write; WSL's sparse VHD; Recall).
- **`suggest`**: **runs** (`from_tweaks`, `from_system` and `from_leftovers` are the Mac's alone;
  `suggest::windows_tests`, and a CI step that checks it says nothing of a Mac's). Still open:
  causes for more Windows folders, the tweaks once they exist, a Windows `from_system` (restore
  points' shadow storage, the page file, WSL's disks), and a keep-list check for kept files inside
  a temp folder.
- **Backups page** (the Time Machine twin): System Restore points and shadow storage, rebuildable
  folders inside OneDrive-backed Desktop/Documents. Design it with the author first.
- **Apps not opened in months**: Uninstall keys and UserAssist (verify first); the action opens
  `ms-settings:appsfeatures`, never an uninstaller.
- **Old files**: done. If the drive doesn't record last access (`fs::records_last_access`: NTFS
  and `NtfsDisableLastAccessUpdate`), "old" is what nothing changed, and the report says so
  (`last_opened_unknown`); Windows' own rules for what's yours in `space::windows::own_file`.
  Tested on Windows by `clean::windows_tests` (CI).
- **Ask AI** on Windows: written. The token file has an access list of the user's alone
  (`private.rs`), checked on every read; the version is Windows' (`sys::windows_version`); the AI
  is told it's a PC; generic names follow Windows' places (`knowledge::generic_on_windows`); the
  window has it (Settings, Storage's items). CI runs the stand-in OpenAI tests on Windows; a real
  sign-in from a PC is the author's to try. Still open: Windows entries in the knowledge list.
- **`writers` and per-process churn**: the Windows write counter includes pipes (measured), so
  writers are not recorded (`WRITES_ARE_DISK_WRITES = false`). Needs evidence from the disk (journal
  activity under the process's folders, or the service's ETW disk I/O) before it is switched on.
- `mdif docker`/`timemachine` stay refused until their halves exist; `timemachine` may become
  the backups page above.

## Open in Phase 2 (known gaps, all listed in `docs/WINDOWS.md`)

- Moving to the Recycle Bin (`fs::trash`): written (`shell.rs`), to be run on a real runner,
  the too-big refusal included (its default size is Windows' documented one, from 2014: a bin
  never yet set has no `MaxCapacity`).
- Children are opened by path (`dir.join(name)`) for link counts and the held-open refresh, not
  relative to the checked folder handle (`NtCreateFile` with a root directory).
- ReFS: 128-bit file IDs, version-3 journal records, block clones.
- The journal position is checked as an administrator only, not as a standard account. A folder
  the account can't open is left out of a replay, not counted as a gap.
- The owner tracker's gap on Windows (`docs/WINDOWS.md`): a file moved into an old folder seen
  for the first time (the Recycle Bin's, at the first deletion after the watcher starts) isn't
  credited there; that needs the journal's rename records per file, or listing the bin at the
  start. (Fixed: a file held open across a watcher restart is followed again; a folder that moves
  and changes in one period takes its size along; first sightings no longer use up the period's
  budget for known folders; a file directly in the profile folder or a drive's root is an owner
  of its own; a folder made since the last period credits all it holds; a rescan cut short is a
  gap, and the last gap is in `status --json` and `owners --json`.)
- A mapped network drive has no volume GUID, so its history is keyed by its letter.
- If the disk under a running watcher is swapped, folder tracking stops but open incidents stay open
  until a restart.
- The unprivileged journal read stands in for ETW, so write-churn needs the service (Phase 5).

## Phase 5: optional elevated service

`mdif service install|uninstall|status` (one UAC prompt), SCM through `windows-sys` (the
`windows-service` crate is not in the offline cache), a named pipe only the installing user can open,
a fixed set of verbs, nothing free-form: journal at full precision, MFT enumeration for a fast
complete `space`/`map`, ETW disk I/O per process, admin clean-ups re-checked with `clean::plan`
(restore points, DISM component cleanup, `Windows.old`, hibernation, Delivery Optimization cache).
**A security review comes before any release that includes it.**

## Phase 6: the Tauri app (`app-windows/`)

**Started** (what it is: `docs/WINDOWS.md`, "The app and its installer"). Done: the tray (free
space and what fills it in the tooltip), the window hidden to the tray, single instance, a port of
`Core.run` (`mdif.rs`, a fixed list of commands), the monitor and its notifications by the Mac's
rules (`monitor.rs`, `news()` tested), the welcome (start the watcher; Get Started sets up opening
at sign-in, `login.rs`), and the pages Overview (RailBar in SVG, Needs Attention, Recommendations, Recent Changes), Storage (kinds
and their items, with clean-ups through `mdif clean`), History (Space Used with the forecast, clean-ups,
Writes, left out on Windows with the write-churn alert: no writers are recorded there), the Clean Up
sheet (the Mac's list, ticks and one-after-another run; `cleanUpRows` tested), Storage's Folders
view (the disk map: sunburst, treemap and outline from `mdif map`, kept in the app's cache folder,
measured while on screen; `map.js` tested) and Settings
(watching, sign-in, notifications, alert choices, the watcher's measured cost), and launchd's part:
a watcher that ended though it's set to start at sign-in is started again (`Supervisor`: seen ended
twice two minutes apart, at most hourly, never one stopped on purpose with `mdif agent stop`, whose
`stopped` marker any start clears; CI kills the watcher and sees the app start it). Tests: `cargo test` in
`app-windows/src-tauri`, `node --test app-windows/ui/tests` (the window's logic against
`app/Tests`, and `pages.test.mjs`, which builds the Prevention page on a stand-in document: a
click on a switch's row must flip the switch, never press Tidy Now); both were checked by breaking
what they guard. Seen offscreen in headless Chrome
against Windows-like fixtures, light and dark; not yet run on a real Windows PC (CI starts it on the
runner).

Still open:
- Pages: Backups (design first). Old Files, Ask AI and Prevention are done (written, previewed;
  not yet run on a PC).
- Spikes: toasts from the installed, unsigned app; the Task Manager switch flipped by hand; WebView2
  on an older Windows 10.
- The author clicks through it on a real PC.

## Phase 7: packaging and release

**Started.** `app-windows/build.sh [target]` builds the NSIS installer (per user, no administrator)
with `mdif.exe` and `mdif-watch.exe` (`tauri.release.json`'s `externalBin`); on Windows with the
Tauri CLI, or from a Mac with `cargo-xwin` and `makensis` (built this way: 3.3 MiB, the three
programs x64, the watcher and the app GUI programs, `mdif` a console one). `src-tauri/windows/hooks.nsh`
stops the watcher before an update and starts it after (`mdif agent stop|start`), and runs
`mdif agent uninstall` on an uninstall; "Delete the application data" also removes the history.
CI's `windows-app` job installs, updates over a running watcher and uninstalls it, and keeps the
installer.

Still open: Authenticode signing (the author picks a certificate or Azure Trusted Signing, then
`bundle > windows > signCommand`); the updater (`tauri-plugin-updater` 2.14.1 is in the cache; its
key is generated once by the author, like the Sparkle one, the private key kept off the repo and
the public key in `tauri.conf.json`; every update artifact signed in `release.yml`); the feed
(`latest.json`) on the latest *published* release, so only publishing ships; a `release.yml`
Windows job that uploads a draft; a Windows section in the README and the site (`docs/RELEASING.md`
has one: building, the version, what's still to decide); the ARM64 installer run on ARM hardware (CI's `windows-app` builds it
from its x64 runner and `windows-app-arm` runs it on `windows-11-arm`: written, not run yet; `build.sh aarch64-pc-windows-msvc` builds it from a Mac: ARM64 programs, checked by their PE headers; x64 runs emulated on Windows on ARM). A Microsoft Store (MSIX) edition
is out of scope for the first release.

## How to work on it

- Mac, before every push: `cargo fmt --all`, `cargo clippy --offline --all-targets -- -D warnings`,
  `cargo test --offline --workspace`.
- Windows type-check from the Mac (also `aarch64`):
  `LIBSQLITE3_SYS_USE_PKG_CONFIG=1 cargo clippy --offline --workspace --all-targets --target
  x86_64-pc-windows-msvc -- -D warnings`. MSRV: `RUSTFLAGS="-D warnings"
  LIBSQLITE3_SYS_USE_PKG_CONFIG=1 cargo +1.88 check --offline --workspace --all-targets --target
  x86_64-pc-windows-msvc`. CI's clippy is newer than local 1.95, so trust CI for new lints.
- The app, from `app-windows/src-tauri`: `cargo fmt --check`, `cargo clippy --offline --all-targets
  -- -D warnings`, `cargo test --offline`; for Windows the same clippy with `--target
  x86_64-pc-windows-msvc` (and `aarch64`) and LLVM's `llvm-rc` on the `PATH`. The window:
  `node --test app-windows/ui/tests`, and the page in a browser (any static server at the repo's
  root, then `app-windows/ui/index.html`, with `?fixtures=` for other test files). The installer:
  `app-windows/build.sh`.
- Real Windows: `git push origin windows-port` (SSH), then `gh workflow run ci.yml --ref windows-port`
  (CI only triggers on main and pull requests). Measurements: `-f spikes=true`.
- Review every commit with Codex, from a checkout of that commit:
  `node "$(ls -d ~/.claude/plugins/cache/openai-codex/codex/*/scripts/codex-companion.mjs | tail -1)"
  review --wait --base HEAD~1`. (A branch-wide review is not a per-commit one: to review an older
  commit, `git worktree add --detach <dir> <sha>` and run it there with `--base <sha>~1`.) Fix the P1/P2s that are real, write down the
  rest in `docs/WINDOWS.md` known gaps. Never amend a pushed commit.
- Known flakes, not caused by this work: `timemachine::tests::a_tool_says_which_of_its_folders_are_caches`
  under load; the macOS `e2e` job (`fill folder grew`, or two churn episodes on a slow runner): rerun
  the failed job.
- Windows tests never touch the real user's settings or registry: temp dirs and scratch keys.
- Public repo: no personal details (project names, paths) in committed docs.

## Before merging to `main`

The branch touches shared code (`watch.rs`, `owners.rs`, `suggest.rs`, the CLI, `Cargo.toml`s). The
Mac tests, the e2e and the app jobs pass on it; still do a Mac smoke test of the app build
(`app/build.sh`, `app/check-contract.sh`) and a look at `git diff main --stat` before opening the pull
request. `mdif-watch` is built on macOS too (same source, harmless): decide whether to keep that or
gate it.

## Review record

Every commit on the branch has had a Codex review against its parent, either as it was made or, for
the early ones and the docs commits, afterwards from a detached worktree at that commit. What those
afterwards-reviews found was either already fixed by a later commit or is listed above and in
`docs/WINDOWS.md` (history keys of Windows histories made before the volume-GUID key, and locks of
watchers started by older builds, are not handled: nothing was released).
