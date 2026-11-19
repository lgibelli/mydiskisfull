//! Thin, safe wrappers over the macOS system calls MyDiskIsFull needs.
//!
//! - [`bulk`]: directory listing with APFS private size, clone family and
//!   cloud-placeholder flags (`getattrlistbulk`).
//! - [`changes`]: what changed since a saved position (FSEvents behind a
//!   neutral face).
//! - [`extents`]: where a file's data physically lives, to count blocks
//!   shared by modified clones once.
//! - [`fsevents`]: which directories changed, live or replayed from a saved
//!   event ID.
//! - [`fs`]: volume statistics and the per-process switch that stops cloud
//!   placeholders from being downloaded while we look at them.
//! - [`proc`]: bytes read/written per process (`proc_pid_rusage`, no root).
//! - [`sys`]: when the Mac started, how much swap it holds, Full Disk Access,
//!   and standard error into a log.

// This crate is empty on every other OS (it only stops being a build error
// by itself). Whatever uses it picks the backend for its own target.
#![cfg(target_os = "macos")]

pub mod bulk;
pub mod changes;
pub mod extents;
pub mod fs;
pub mod fsevents;
pub mod proc;
pub mod sys;
