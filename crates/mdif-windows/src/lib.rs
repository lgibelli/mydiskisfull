//! Thin, safe wrappers over the Windows calls MyDiskIsFull needs: the same
//! modules and functions as `mdif-macos`, which is its twin.
//!
//! - [`bulk`]: directory listing with sizes, times and cloud-placeholder
//!   flags.
//! - [`changes`]: what changed since a saved position (not read yet: every
//!   replay is a gap).
//! - [`extents`]: where a file's data physically lives (not on Windows: NTFS
//!   has no clones).
//! - [`fs`]: volume statistics, file identity, locks, and the per-process
//!   switch that stops cloud placeholders from being downloaded.
//! - [`proc`]: processes: their I/O, their programs, ending them.
//! - [`startup`]: what starts at login (the `Run` key and Task Manager's
//!   switch for it); Windows only.
//! - [`sys`]: when the PC started, the user's folders, the system folder,
//!   standard error into a log, the background priority.
//!
//! Where a call isn't written yet it says so with
//! [`std::io::ErrorKind::Unsupported`] (or an empty list where callers expect
//! one), never with a made-up answer.

// Empty on every other OS: whatever uses it picks the backend for its target.
#![cfg(windows)]

pub mod bulk;
pub mod changes;
pub mod extents;
pub mod fs;
pub mod proc;
pub mod startup;
pub mod sys;

use std::io;

/// Wide, NUL-ended text for a Windows call.
pub(crate) fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(Some(0)).collect()
}

/// The error for what isn't written yet.
pub(crate) fn unsupported(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{what} isn't available on Windows yet"),
    )
}
