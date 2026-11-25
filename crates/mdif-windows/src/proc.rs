//! Processes: this one's cost, others' programs, ending them.
//!
//! Not written yet: what a process has open, an app's helpers, and asking a
//! process to close politely. Those come back empty; everything else here is
//! real. A process's write counter counts every write it makes, pipes and
//! sockets too (measured, Phase 0), so it is the most a process wrote to the
//! disk, not what it did.

use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::{
    GetLengthSid, GetTokenInformation, IsValidSid, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessIoCounters, GetProcessTimes, IO_COUNTERS, OpenProcess,
    OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
};

use crate::unsupported;

/// `ERROR_ACCESS_DENIED`: it exists, and isn't ours to look at.
const ACCESS_DENIED: i32 = 5;

/// Whether [`ProcIo::bytes_written`] is bytes written to the disk. Here it isn't:
/// pipes and sockets count too (measured, Phase 0), so a process that only
/// sends over the network looks like a heavy writer. The watcher records
/// writers only where it is true, until Windows has evidence from the disk.
pub const WRITES_ARE_DISK_WRITES: bool = false;

#[derive(Debug, Clone)]
pub struct ProcIo {
    pub pid: i32,
    pub name: String,
    /// Cumulative since the process started.
    pub bytes_read: u64,
    pub bytes_written: u64,
}

/// I/O counters of every process owned by the current user: the ones whose
/// token names the same account as this process's, and whose counters can be
/// read. The name is the program's, without `.exe`.
pub fn own_processes() -> Vec<ProcIo> {
    // SAFETY: the pseudo handle of this process needs no closing.
    let Some(me) = token_user(unsafe { GetCurrentProcess() }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (pid, exe) in all_processes() {
        let Ok(p) = Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
            continue;
        };
        if token_user(p.0).as_deref() != Some(&me[..]) {
            continue;
        }
        // SAFETY: zeroed is a valid IO_COUNTERS; the handle is open for queries.
        let mut io: IO_COUNTERS = unsafe { std::mem::zeroed() };
        if unsafe { GetProcessIoCounters(p.0, &mut io) } == 0 {
            continue;
        }
        out.push(ProcIo {
            pid,
            name: program_name(&exe),
            bytes_read: io.ReadTransferCount,
            bytes_written: io.WriteTransferCount,
        });
    }
    out
}

/// `chrome.exe` is `chrome`, as the Mac names a process.
fn program_name(exe: &str) -> String {
    let lower = exe.to_ascii_lowercase();
    match lower.strip_suffix(".exe") {
        Some(_) => exe[..exe.len() - 4].to_string(),
        None => exe.to_string(),
    }
}

/// Every process now: its id and the name of its program.
fn all_processes() -> Vec<(i32, String)> {
    // SAFETY: plain call; the snapshot handle is closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut out = Vec::new();
    // SAFETY: zeroed is a valid PROCESSENTRY32W once `dwSize` is set, as the
    // calls need; `snap` is open for as long as the loop runs.
    unsafe {
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snap, &mut e) != 0;
        while more {
            let len = e
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(e.szExeFile.len());
            let name = std::ffi::OsString::from_wide(&e.szExeFile[..len])
                .to_string_lossy()
                .into_owned();
            // Process 0 is the idle process: no one's.
            if e.th32ProcessID != 0 {
                out.push((e.th32ProcessID as i32, name));
            }
            more = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}

/// The account `process` runs as: its user's SID, as bytes to compare.
fn token_user(process: HANDLE) -> Option<Vec<u8>> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `process` is open for queries; the token is closed below.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let mut need = 0u32;
    // 8-byte aligned, as TOKEN_USER is.
    let mut buf = vec![0u64; 64];
    // SAFETY: `buf` is as long as the length passed; `token` is open for queries.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast(),
            (buf.len() * 8) as u32,
            &mut need,
        )
    };
    // SAFETY: the token is ours, and closed once.
    unsafe { CloseHandle(token) };
    if ok == 0 {
        return None;
    }
    // SAFETY: on success `buf` starts with a TOKEN_USER whose SID follows it
    // inside the bytes written: checked before the SID is read, its fixed
    // part (8 bytes) first, then the length it says it has.
    unsafe {
        let user = &*buf.as_ptr().cast::<TOKEN_USER>();
        let sid = user.User.Sid;
        let start = buf.as_ptr() as usize;
        let end = start + (need as usize).min(buf.len() * 8);
        let at = sid as usize;
        if at < start || at + 8 > end || IsValidSid(sid) == 0 {
            return None;
        }
        let len = GetLengthSid(sid) as usize;
        if at + len > end {
            return None;
        }
        Some(std::slice::from_raw_parts(sid.cast::<u8>(), len).to_vec())
    }
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

/// The program an open, live process runs.
fn image(p: &Process) -> Option<PathBuf> {
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

/// The program `pid` runs, while it runs: an ended process whose object is
/// still held open by someone can still be asked, and has none.
pub fn path(pid: i32) -> Option<PathBuf> {
    image(&Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE).ok()?)
}

/// Ends `pid` if it runs `exe`: the program is looked at, and the process ended,
/// through one handle, so a process id that was reused since is never touched.
/// `Ok(false)` if it isn't running that program (or has ended already).
pub fn terminate_program(pid: i32, exe: &std::path::Path) -> io::Result<bool> {
    let p = Process::open(
        pid,
        PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE | PROCESS_TERMINATE,
    )?;
    let Some(ran) = image(&p) else {
        return Ok(false);
    };
    if !ran
        .to_string_lossy()
        .eq_ignore_ascii_case(&exe.to_string_lossy())
    {
        return Ok(false);
    }
    // SAFETY: the handle is open, with the right to terminate.
    if unsafe { TerminateProcess(p.0, 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(true)
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
    fn a_process_is_ended_only_if_it_runs_the_program_asked_for() {
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let ping = path(pid).unwrap();
        // Another program's name: left alone.
        let other = std::env::current_exe().unwrap();
        assert!(!terminate_program(pid, &other).unwrap());
        assert!(alive(pid));
        // Its own, in any case: ended.
        let shouted = std::path::PathBuf::from(ping.to_string_lossy().to_uppercase());
        assert!(terminate_program(pid, &shouted).unwrap());
        child.wait().unwrap();
        // Ended already: nothing to do, not an error.
        assert!(!terminate_program(pid, &ping).unwrap_or(false));
    }

    #[test]
    fn an_ended_process_is_not_alive() {
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
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
    fn a_program_is_named_without_its_extension() {
        assert_eq!(program_name("Chrome.EXE"), "Chrome");
        assert_eq!(program_name("node"), "node");
        assert_eq!(program_name("a.exe.exe"), "a.exe");
    }

    #[test]
    fn the_users_processes_include_this_one_and_its_child() {
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let me = std::process::id() as i32;
        let all = own_processes();
        child.kill().unwrap();
        child.wait().unwrap();
        let mine = all.iter().find(|p| p.pid == me).expect("this process");
        assert!(mine.bytes_written > 0 || mine.bytes_read > 0, "{mine:?}");
        assert!(
            all.iter()
                .any(|p| p.pid == child.id() as i32 && p.name.eq_ignore_ascii_case("ping"))
        );
        // Windows's own processes aren't the user's.
        assert!(!all.iter().any(|p| p.pid == 4), "the System process");
    }

    #[test]
    fn what_isnt_written_yet_is_empty_or_says_so() {
        assert!(files_open_for_writing(1).is_empty());
        assert_eq!(responsible_app(1), None);
        assert_eq!(terminate(1).unwrap_err().kind(), io::ErrorKind::Unsupported);
    }
}
