# Finish-up plan: the Windows port

Branch `windows-port`, pushed, CI green on every job (macOS, real Windows runner, Windows on ARM,
MSRV 1.88). The long-form plan and findings are in `docs/WINDOWS.md`; this file is what is left.

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
| 6 | Tauri app | not started |
| 7 | Packaging, signing, updater | not started |

What runs on Windows today: `watch`, `status`, `forecast`, `alerts`, `keep`, `knowledge` (no Ask AI),
`agent`, `owners`, `incidents`, `map`. Still refused with "isn't available on this operating system
yet" (`NOT_HERE_YET` in `crates/mdif-cli/src/main.rs`): `suggest`, `clean`, `tidy`, `docker`,
`timemachine`, `tweaks`, `space`, `writers`. When a command's Windows half lands: remove its name from
that list, remove its `#[command(hide = !cfg!(target_os = "macos"))]` (the commands are hidden from
`--help` on Windows until then), and remove it from the refusal loop in `ci.yml`.

## `mdif map` on Windows

Done, with the findings of its review: estimates are said in words (`notes` in the JSON, `note:` in
the text: small hard links, files held open for writing, a disk quota), a ReFS root is refused, a quota
account gets no reconciliation, a partly downloaded cloud file counts what is resident, and the
reconciliation is shown for any drive's root. Still open: hard-link counts for files under 1 MiB and
a fresh size for files held open (both are noted, not measured), and a snapshot/restore-point line.

## Phase 4: the Windows catalogue (parity with the Mac)

Each item goes through its own engine and its own tests with temp dirs (and a scratch registry key),
never the user's real settings.

- **`space`**: the whole disk sorted into kinds. System files (WinSxS, Installer, SoftwareDistribution,
  `pagefile.sys`, `hiberfil.sys`), leftovers (`Windows.old`, `$Recycle.Bin`), Program Files and
  ProgramData, the profile (AppData Local/Roaming, Packages, Temp), dev folders (`.gradle`, npm,
  `.nuget`, `.cargo`, Android), WSL and Docker `.vhdx`, VMs. What can't be read goes to "System data".
  `space::purgeable` is 0 off macOS today.
- **`handlers`**: what apps keep where (chat, mail OST/PST explained and never deleted, iPhone backups
  under MobileSync, game libraries, browsers, OneDrive).
- **`clean` and `tidy`**: Recycle Bin (needs `fs::trash`, a stub today), empty the Recycle Bin,
  build output, emptying a cache while its app is closed (checked by process image path, using
  `proc::terminate_program`'s identity check), Docker (`docker.exe`), Android. Add `temp`
  (`%TEMP%` older than 7 days *and* not in use, not on the keep list, and re-checked by `clean::plan`:
  age alone is not enough, a program may still need what it wrote last week). Every new kind goes through `clean::plan` and `clean::run`, which
  re-check each path. Simulators and backups drop out.
- **`tweaks`**: `Setting::Registry` and `Setting::Ini`, the Windows catalogue (Storage Sense and
  Recycle Bin days, Chrome/Edge model policies under `HKCU\Software\Policies`, WSL sparse VHD,
  Recall, Gradle/Claude carry over). `startup.rs` already has the registry primitives.
- **`suggest`**: causes for Windows folders (the generic advice and the Temp case exist); needs the
  catalogue above and a keep-list check for kept files inside a temp folder.
- **Backups page** (the Time Machine twin): System Restore points and shadow storage, rebuildable
  folders inside OneDrive-backed Desktop/Documents. Design it with the author first.
- **Apps not opened in months**: Uninstall keys and UserAssist (verify first); the action opens
  `ms-settings:appsfeatures`, never an uninstaller.
- **Old files**: if `NtfsDisableLastAccessUpdate` is on, "old" uses mtime alone, and the report says so.
- **Ask AI** on Windows: needs an ACL-protected token file (`portable::private_files()` is false
  there, so sign-in is refused), a Windows version string instead of `sw_vers`, and the prompts and
  knowledge entries that name macOS folders and apps need Windows folder names (AppData, Program
  Files, Packages) so answers aren't about the wrong system.
- **`writers` and per-process churn**: the Windows write counter includes pipes (measured), so
  writers are not recorded (`WRITES_ARE_DISK_WRITES = false`). Needs evidence from the disk (journal
  activity under the process's folders, or the service's ETW disk I/O) before it is switched on.
- `mdif tidy`/`docker`/`timemachine` stay refused until their halves exist; `timemachine` may become
  the backups page above.

## Open in Phase 2 (known gaps, all listed in `docs/WINDOWS.md`)

- Recycle Bin (`fs::trash`), still a stub.
- Children are opened by path (`dir.join(name)`) for link counts and the held-open refresh, not
  relative to the checked folder handle (`NtCreateFile` with a root directory).
- ReFS: 128-bit file IDs, version-3 journal records, block clones.
- The journal position is checked as an administrator only, not as a standard account.
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

Tauri 2.12.6 and the plugins are in the offline cache. Tray icon (free space in the tooltip) plus a
window; a port of `Core.run` (runs `mdif`, `--json` last); polling as in `Monitor.swift`; toasts;
single-instance; updater. UI is plain HTML/CSS/JS, no bundler or npm packages, JSDoc types mirroring
`app/Sources/Model.swift`. Pages as on the Mac (Overview, Storage + Folders map, Old Files, History,
Backups, Prevention, Clean Up, Settings, Ask AI); the **RailBar is redrawn in SVG and never replaced**;
what an action did goes to a notices banner, only the question before what can't be undone is a
dialog. A node contract check decodes `app/Tests` fixtures and live `mdif --json` on the runner.
Dev loop on the Mac: `cargo tauri dev` against the Mac `mdif`, and a fixture mode for screenshots.
Unmeasured spikes still to do first: toasts from an unpackaged exe, WebView2 presence, the
`StartupApproved` behaviour with a real Task Manager toggle, cloud placeholders (needs a sync root).

## Phase 7: packaging and release

Tauri NSIS per-user installer (no admin) with `mdif.exe` and `mdif-watch.exe`; Authenticode signing
(the author picks a certificate or Azure Trusted Signing); the Tauri updater's own signing key (like
the Sparkle one: generated once, private key kept off the repo, public key in the app, every update
artifact signed in `release.yml`); updater feed (`latest.json`) on the latest *published* release, so
only publishing ships; a `release.yml` Windows job that uploads a draft; a
Windows section in `docs/RELEASING.md`, the README and the site. A Microsoft Store (MSIX) edition is
out of scope for the first release. The author installs it on a real Windows PC and clicks through:
that is the only step that needs a box outside CI.

## How to work on it

- Mac, before every push: `cargo fmt --all`, `cargo clippy --offline --all-targets -- -D warnings`,
  `cargo test --offline --workspace`.
- Windows type-check from the Mac (also `aarch64`):
  `LIBSQLITE3_SYS_USE_PKG_CONFIG=1 cargo clippy --offline --workspace --all-targets --target
  x86_64-pc-windows-msvc -- -D warnings`. MSRV: `RUSTFLAGS="-D warnings"
  LIBSQLITE3_SYS_USE_PKG_CONFIG=1 cargo +1.88 check --offline --workspace --all-targets --target
  x86_64-pc-windows-msvc`. CI's clippy is newer than local 1.95, so trust CI for new lints.
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
