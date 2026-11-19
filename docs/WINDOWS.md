# MyDiskIsFull on Windows

The same watcher, forecast and explanations, for NTFS. This is the plan and where it stands.
`docs/PLAN.md` is the product; this file is only what Windows changes.

## How it is built

Everything the operating system does sits behind `mdif_core::os`, which re-exports the one
backend of the target and pins each function's signature, so the two can't drift:

| | macOS | Windows |
|---|---|---|
| Crate | `crates/mdif-macos` | `crates/mdif-windows` (on `windows-sys`) |
| Listing a folder | `getattrlistbulk` | the find data (an open handle with `FileIdExtdDirectoryInfo` is next) |
| Change journal | FSEvents, replayed from a saved ID | not read yet; then the NTFS journal, or live notifications |
| Per-process writes | `proc_pid_rusage` | `GetProcessIoCounters` (not written yet) |
| Cloud placeholders | `setiopolicy_np`, `SF_DATALESS` | `FILE_FLAG_OPEN_NO_RECALL`, the recall attributes |
| Volumes | `statfs`, APFS containers | `GetDiskFreeSpaceExW`, volume GUIDs |

`mdif_core::portable` holds what `std` only gives on Unix (allocated bytes, file identity, link
count, owner, file modes). The rest of `mdif-core` is one copy of the code on both systems.

The CLI is `mdif` / `mdif.exe` with the same commands and JSON. The Windows app (Tauri) will run
it the way the Mac app does, and `app/Tests` holds the contract.

## Where it stands

Phase 1 (done): the workspace builds, lints and tests on x86_64 Windows (built and run on a
runner), is type-checked for aarch64 Windows (not run), and the Mac is unchanged. What runs on Windows today is the free-space forecast: `mdif watch`, `status`,
`forecast`, `alerts`, `keep` and `knowledge` (without Ask AI). The rest says it isn't available on
this operating system yet, rather than printing numbers built on another system's picture of a
disk: `suggest`, `clean`, `tidy`, `docker`, `timemachine`, `tweaks`, `agent`, `map`, `space`,
`writers`, `owners` and `incidents`.

| Phase | What | State |
|---|---|---|
| 0 | Spikes on the runner: the NTFS journal without admin, listing speed, whether network I/O counts as writes (placeholders and toasts still to measure) | measured, below |
| 1 | The seam: builds and tests on Windows, no change on the Mac | done |
| 2 | The backend for real: listing from a handle (allocated size, file number, links), process list and writes, Recycle Bin, moved folders by number | |
| 3 | The watcher and trends: the change journal, per-process writes, the agent (a `Run` key), the e2e test on an NTFS disk image | |
| 4 | Windows' own catalogue: what fills a disk there, handlers, clean-ups, tidy-ups, tweaks (registry), the backups page | |
| 5 | An optional elevated service for the journal at full precision and the file table | |
| 6 | The app (Tauri) | |
| 7 | Packaging (a per-user installer, signing, updates) | |

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
| NTFS journal without administrator rights | With a restricted token (an administrator's filtered token, the closest a runner gets to a standard user): the volume device opens with `FILE_TRAVERSE`, and `FSCTL_QUERY_USN_JOURNAL` and `FSCTL_READ_UNPRIVILEGED_USN_JOURNAL` work, on the volume device and on a handle to `C:\` opened with no access at all. `FSCTL_READ_USN_JOURNAL` is denied (error 5). | Reading the journal needs no service and no UAC. That a change can be found again by replaying from a saved USN and journal id isn't shown yet: the probe read from the first retained record, and the replay test (save a cursor, change a known file, find it) is Phase 3's. The elevated service (Phase 5) is then only for full precision. The restricted token is an approximation: it still needs checking as a real standard account. |

Not measured yet: cloud placeholders (needs a sync root), toasts from an unpackaged program, the
`StartupApproved` behaviour of a `Run` key, WebView2.

## Known gaps, to carry into the next phases

- The listing is read from one open, checked folder handle (`FileIdExtdDirectoryInfo`): allocated
  size, file number and reparse tag come with it, and every record is bounds-checked before it is
  read. Still open:
  - The link count isn't in the listing, so only files of 1 MiB or more are asked for it, by
    opening them by path (`dir.join(name)`), which looks the name up again. Opening the child
    relative to the checked folder handle (`NtCreateFile` with a root directory) closes that gap.
  - A count that can't be had is `LINKS_UNKNOWN`: the scanner deduplicates the file but never
    counts it as wholly ours, and the owner tracker splits it by two.
  - A file with several names shows the size of the name that was listed; another name's entry
    can be stale until it's closed. A handle's own size would be current.
  - The file number is the low 8 bytes of the 128-bit ID. NTFS only uses that much; on ReFS two
    siblings can share it, so ReFS volumes have to keep the whole ID before they are scanned.
  Until these are closed nothing that scans or sizes folders runs on Windows.
- `own_processes` lists the user's processes (token's SID compared with ours) with their I/O
  counters, but a Windows write counter counts pipes and sockets, so the watcher records no
  writers (`WRITES_ARE_DISK_WRITES` is false) and `writers` stays refused, until churn has
  evidence from the disk (changes under the process's folders, or the service's ETW disk I/O).
- History is keyed by the volume's identity (`FsStats::history_key`: its GUID), and the watcher
  re-checks it on every sample, so a disk swapped under a drive letter starts its own history.
- The owner tracker's rules are rooted at `/` and at macOS's places in the home folder, and the
  space, keep and knowledge code strips `/System/Volumes/Data` and joins `/`: Windows paths need
  drive prefixes and their own categories.
- Signing in with ChatGPT (and so Ask AI) is refused: its token file can only be made readable by
  its owner with an access-control list, which isn't written. Ask AI also names the macOS version.
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
lints, tests, builds and smoke-tests on a real runner, a `windows-arm` job for aarch64, and the
MSRV job type-checks Windows on Rust 1.88.
