//! The operating system's side of the disk: listing folders with their real
//! sizes, volume space, per-process writes, the change journal. One backend
//! per target, picked here; everything above this module is the same code
//! on every OS.
//!
//! - macOS: `mdif-macos` (`getattrlistbulk`, FSEvents, `proc_pid_rusage`).
//! - Windows: `mdif-windows`.
//!
//! The backends are twins: the checks below are what the code above uses of
//! them, so one that drifts fails to build here, not somewhere far from it.

#[cfg(target_os = "macos")]
pub use mdif_macos::{bulk, changes, extents, fs, proc, sys};
#[cfg(windows)]
pub use mdif_windows::{bulk, changes, extents, fs, proc, startup, sys};

#[cfg(not(any(target_os = "macos", windows)))]
compile_error!("MyDiskIsFull has no backend for this operating system yet");

#[cfg(any(target_os = "macos", windows))]
mod twins {
    use super::*;
    use std::fs::File;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    const _: fn(&Path) -> io::Result<fs::FsStats> = fs::statfs;
    const _: fn(&fs::FsStats) -> String = fs::FsStats::history_key;
    const _: fn() -> io::Result<Vec<(String, String)>> = fs::mounts;
    const _: fn(&Path) -> io::Result<u64> = fs::space_used;
    const _: fn(&Path) -> io::Result<fs::FsId> = fs::fsid;
    const _: fn(fs::FsId, u64) -> io::Result<PathBuf> = fs::path_of;
    const _: fn(&Path) -> io::Result<i32> = fs::dev_of;
    const _: fn(&Path) -> io::Result<PathBuf> = fs::trash;
    const _: fn(&Path) -> io::Result<u64> = fs::available_for_important_usage;
    const _: fn(&Path, bool) -> io::Result<()> = fs::set_excluded_from_backup;
    const _: fn(&Path) -> io::Result<bool> = fs::excluded_from_backup;
    const _: fn(&Path, &str, bool) -> io::Result<()> = fs::set_mark;
    const _: fn(&Path, &str) -> io::Result<bool> = fs::has_mark;
    const _: fn(&File) -> io::Result<()> = fs::lock;
    const _: fn() -> io::Result<()> = fs::forbid_dataless_materialization;

    const _: u32 = bulk::LINKS_UNKNOWN;
    const _: fn(&Path) -> io::Result<Vec<bulk::Entry>> = bulk::read_dir;
    const _: fn(&Path, bulk::Detail) -> io::Result<Vec<bulk::Entry>> = bulk::read_dir_with;
    const _: fn(&bulk::Entry) -> bool = bulk::Entry::is_dataless;
    const _: fn(&bulk::Entry) -> bool = bulk::Entry::may_share_blocks;
    const _: fn(&bulk::Entry) -> u64 = bulk::Entry::shared;

    #[allow(clippy::type_complexity)]
    const _: fn(&Path, u64) -> io::Result<Vec<(u64, u64)>> = extents::physical;

    const _: fn(&Path) -> u64 = changes::now;
    const _: fn(&Path, u64, Duration) -> io::Result<changes::Replay> = changes::replay;

    const _: bool = proc::WRITES_ARE_DISK_WRITES;
    const _: fn() -> Vec<proc::ProcIo> = proc::own_processes;
    const _: fn() -> (Duration, u64) = proc::own_usage;
    const _: fn(i32) -> Vec<PathBuf> = proc::files_open_for_writing;
    const _: fn(i32) -> Option<proc::App> = proc::responsible_app;
    const _: fn(i32) -> io::Result<()> = proc::terminate;
    const _: fn(i32) -> io::Result<()> = proc::force_quit;
    const _: fn(i32) -> bool = proc::alive;
    const _: fn(i32) -> Option<PathBuf> = proc::path;

    const _: fn(&Path) -> Vec<PathBuf> = sys::access_probes;
    const _: fn(&[PathBuf]) -> Option<bool> = sys::full_disk_access;
    const _: fn() -> Option<PathBuf> = sys::account_home;
    const _: fn(&File) -> io::Result<()> = sys::redirect_stderr;
    const _: fn() -> io::Result<i64> = sys::boot_time;
    const _: fn() -> io::Result<u64> = sys::swap_used;
    const _: fn(i64) -> i64 = sys::utc_offset;
    const _: fn() -> io::Result<sys::Background> = sys::background;
    const _: fn() -> bool = sys::in_background;
}
