//! Thin, safe wrappers over the macOS system calls MyDiskIsFull needs.
//!
//! - [`bulk`]: directory listing with APFS private size, clone family and
//!   cloud-placeholder flags (`getattrlistbulk`).
//! - [`fs`]: volume statistics and the per-process switch that stops cloud
//!   placeholders from being downloaded while we look at them.
//! - [`proc`]: bytes read/written per process (`proc_pid_rusage`, no root).

#[cfg(not(target_os = "macos"))]
compile_error!("mdif-macos only builds on macOS");

pub mod bulk;
pub mod fs;
pub mod proc;
