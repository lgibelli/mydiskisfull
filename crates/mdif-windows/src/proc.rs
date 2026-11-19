//! Processes: this one's cost, others' programs, ending them.
//!
//! Not written yet: the list of the user's processes with their write
//! counters, what a process has open, an app's helpers, and asking a process
//! to close politely. Those come back empty, and the watcher then has no
//! writers to tell apart; everything else here is real.

use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_TIMEOUT};
use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessIoCounters, GetProcessTimes, IO_COUNTERS, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW,
    TerminateProcess, WaitForSingleObject,
};

use crate::unsupported;

/// `ERROR_ACCESS_DENIED`: it exists, and isn't ours to look at.
const ACCESS_DENIED: i32 = 5;

#[derive(Debug, Clone)]
pub struct ProcIo {
    pub pid: i32,
    pub name: String,
    /// Cumulative since the process started.
    pub bytes_read: u64,
    pub bytes_written: u64,
}

/// I/O counters of every process owned by the current user. Not written yet.
pub fn own_processes() -> Vec<ProcIo> {
    Vec::new()
}

/// An app, by where it lives on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub pid: i32,
    pub name: String,
    pub bundle: PathBuf,
}

/// The app `pid` works for. Not written yet.
pub fn responsible_app(_pid: i32) -> Option<App> {
    None
}

/// Files `pid` has open for writing. Not written yet.
pub fn files_open_for_writing(_pid: i32) -> Vec<PathBuf> {
    Vec::new()
}

/// Asks `pid` to quit. Not written yet.
pub fn terminate(_pid: i32) -> io::Result<()> {
    Err(unsupported("asking a process to quit"))
}

/// A process handle, closed on drop.
struct Process(HANDLE);

impl Process {
    fn open(pid: i32, access: u32) -> io::Result<Process> {
        // SAFETY: plain call; the handle is checked before it's used.
        let h = unsafe { OpenProcess(access, 0, pid as u32) };
        if h.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Process(h))
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: the handle is ours, and closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// CPU time this process has used, and the bytes it has written.
pub fn own_usage() -> (Duration, u64) {
    // SAFETY: the pseudo handle of this process needs no closing; zeroed is
    // a valid value of each plain struct, and the pointers are to them.
    unsafe {
        let me = GetCurrentProcess();
        let zero: FILETIME = std::mem::zeroed();
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        let mut io: IO_COUNTERS = std::mem::zeroed();
        let cpu = if GetProcessTimes(me, &mut created, &mut exited, &mut kernel, &mut user) != 0 {
            // In units of 100 ns.
            let ticks =
                |t: FILETIME| u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime);
            Duration::from_nanos((ticks(kernel) + ticks(user)).saturating_mul(100))
        } else {
            Duration::ZERO
        };
        let written = if GetProcessIoCounters(me, &mut io) != 0 {
            io.WriteTransferCount
        } else {
            0
        };
        (cpu, written)
    }
}

/// Ends `pid` at once, with nothing saved.
pub fn force_quit(pid: i32) -> io::Result<()> {
    let p = Process::open(pid, PROCESS_TERMINATE)?;
    // SAFETY: the handle is open, with the right to terminate.
    if unsafe { TerminateProcess(p.0, 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Whether `pid` is a process that hasn't exited.
pub fn alive(pid: i32) -> bool {
    match Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE) {
        // A process object is signaled once the process has ended, whatever
        // its exit code (which may be 259 too).
        // SAFETY: the handle is open with the right to wait.
        Ok(p) => unsafe { WaitForSingleObject(p.0, 0) == WAIT_TIMEOUT },
        // It exists, and isn't ours to look at.
        Err(e) => e.raw_os_error() == Some(ACCESS_DENIED),
    }
}

/// The program `pid` runs, while it runs: an ended process whose object is
/// still held open by someone can still be asked, and has none.
pub fn path(pid: i32) -> Option<PathBuf> {
    let p = Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE).ok()?;
    // SAFETY: the handle is open with the right to wait.
    if unsafe { WaitForSingleObject(p.0, 0) } != WAIT_TIMEOUT {
        return None;
    }
    let mut buf = vec![0u16; 32_768];
    let mut len = buf.len() as u32;
    // SAFETY: the handle is open for queries; `buf` is `len` long.
    if unsafe { QueryFullProcessImageNameW(p.0, 0, buf.as_mut_ptr(), &mut len) } == 0 {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsString::from_wide(
        &buf[..len as usize],
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_alive_and_runs_this_program() {
        let me = std::process::id() as i32;
        assert!(alive(me));
        let exe = std::env::current_exe().unwrap();
        let ran = path(me).unwrap();
        assert_eq!(
            ran.file_name().map(|n| n.to_ascii_lowercase()),
            exe.file_name().map(|n| n.to_ascii_lowercase())
        );
    }

    #[test]
    fn an_ended_process_is_not_alive() {
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping -n 30 127.0.0.1 >nul"])
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        assert!(alive(pid));
        force_quit(pid).unwrap();
        child.wait().unwrap();
        assert!(!alive(pid));
        assert_eq!(path(pid), None);
    }

    #[test]
    fn cpu_and_writes_grow_with_work() {
        let (cpu0, w0) = own_usage();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("w"), vec![7u8; 1 << 20]).unwrap();
        let mut x = 0u64;
        let t = std::time::Instant::now();
        while t.elapsed() < Duration::from_millis(50) {
            x = x.wrapping_add(std::hint::black_box(1));
        }
        let (cpu1, w1) = own_usage();
        assert!(cpu1 > cpu0, "{cpu0:?} {cpu1:?} {x}");
        assert!(w1 >= w0 + (1 << 20), "{w0} {w1}");
    }

    #[test]
    fn what_isnt_written_yet_is_empty_or_says_so() {
        assert!(own_processes().is_empty());
        assert!(files_open_for_writing(1).is_empty());
        assert_eq!(responsible_app(1), None);
        assert_eq!(terminate(1).unwrap_err().kind(), io::ErrorKind::Unsupported);
    }
}
