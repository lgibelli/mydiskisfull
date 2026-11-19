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
| 0 | Spikes on the runner: the NTFS journal without admin, listing speed, placeholders, whether network I/O counts as writes, toasts | not started |
| 1 | The seam: builds and tests on Windows, no change on the Mac | done |
| 2 | The backend for real: listing from a handle (allocated size, file number, links), process list and writes, Recycle Bin, moved folders by number | |
| 3 | The watcher and trends: the change journal, per-process writes, the agent (a `Run` key), the e2e test on an NTFS disk image | |
| 4 | Windows' own catalogue: what fills a disk there, handlers, clean-ups, tidy-ups, tweaks (registry), the backups page | |
| 5 | An optional elevated service for the journal at full precision and the file table | |
| 6 | The app (Tauri) | |
| 7 | Packaging (a per-user installer, signing, updates) | |

## Known gaps, to carry into the next phases

- A listing reports an entry's length as its allocated size, with no file number and one name.
  A sparse or compressed file is counted too big and hard links twice, and the path is listed
  after it was checked. The real listing from an open handle (`FileIdExtdDirectoryInfo`) gives
  the allocated size and the file number, and fixes the last. It doesn't give the link count, so
  whether another name for a file lies outside the scanned tree is a separate query per file that
  has to be priced (Phase 0) and stays open until then. Until then nothing that scans or sizes
  folders runs on Windows.
- History is keyed by the volume's mount point. On Windows it must be the volume's identity (its
  GUID), so two removable disks that take turns as `E:\` don't share a forecast.
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
