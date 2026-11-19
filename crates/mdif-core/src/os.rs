//! The operating system's side of the disk: listing folders with their real
//! sizes, volume space, per-process writes, the change journal. One backend
//! per target, picked here; everything above this module is the same code
//! on every OS.
//!
//! - macOS: `mdif-macos` (`getattrlistbulk`, FSEvents, `proc_pid_rusage`).

#[cfg(target_os = "macos")]
pub use mdif_macos::{bulk, changes, extents, fs, proc, sys};

#[cfg(not(target_os = "macos"))]
compile_error!("MyDiskIsFull has no backend for this operating system yet");
