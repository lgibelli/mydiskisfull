//! Per-process disk I/O counters (`proc_pid_rusage`). Works without root for
//! the current user's processes, which is what catches runaway writers such
//! as a sync client rewriting gigabytes of logs while its folder size stays
//! flat.

use std::mem::MaybeUninit;

use libc::{c_int, c_void};

// <sys/proc_info.h>
const PROC_PIDT_SHORTBSDINFO: c_int = 13;

#[derive(Debug, Clone)]
pub struct ProcIo {
    pub pid: i32,
    pub name: String,
    /// Cumulative since the process started.
    pub bytes_read: u64,
    pub bytes_written: u64,
}

/// I/O counters of every process owned by the current user.
pub fn own_processes() -> Vec<ProcIo> {
    let me = unsafe { libc::getuid() };
    all_pids()
        .into_iter()
        .filter(|&pid| owner(pid) == Some(me))
        .filter_map(|pid| {
            let (bytes_read, bytes_written) = disk_io(pid)?;
            Some(ProcIo {
                pid,
                name: name(pid),
                bytes_read,
                bytes_written,
            })
        })
        .collect()
}

/// CPU time (user + system) and bytes written by this process so far: what
/// the watcher reports as its own cost.
///
/// Writes are *logical* (`ri_logical_writes`): every byte the process asked
/// to write, including what still sits in the page cache. The disk-I/O
/// counter misses those (SQLite's WAL commits, for one) and would flatter
/// the watcher.
pub fn own_usage() -> (std::time::Duration, u64) {
    let mut ru = MaybeUninit::<libc::rusage>::uninit();
    let cpu = if unsafe { libc::getrusage(libc::RUSAGE_SELF, ru.as_mut_ptr()) } == 0 {
        let ru = unsafe { ru.assume_init() };
        let tv =
            |t: libc::timeval| std::time::Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1_000);
        tv(ru.ru_utime) + tv(ru.ru_stime)
    } else {
        std::time::Duration::ZERO
    };
    let written = rusage(std::process::id() as i32).map_or(0, |ri| ri.ri_logical_writes);
    (cpu, written)
}

// <sys/proc_info.h>: struct vnode_fdinfowithpath, read by offset.
const PROC_PIDFDVNODEPATHINFO: c_int = 2;
const VNODE_FDINFO_WITH_PATH: usize = 1200;
const FI_OPENFLAGS: usize = 0;
const VI_TYPE: usize = 160;
const VIP_PATH: usize = 176;
const FWRITE: u32 = 0x2;
const VREG: i32 = 1;

/// Regular files `pid` has open for writing (the current user's processes,
/// no root needed).
///
/// FSEvents reports a file's folder when the file is created or closed
/// after writing, not while it is written: a log kept open and appended to
/// forever never shows up. Asking the heavy writers what they have open
/// fills that gap.
pub fn files_open_for_writing(pid: i32) -> Vec<std::path::PathBuf> {
    let need =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    if need <= 0 {
        return Vec::new();
    }
    // Room for descriptors opened between the two calls.
    let mut buf = vec![0u8; need as usize + 64 * 8];
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDLISTFDS,
            0,
            buf.as_mut_ptr().cast(),
            buf.len() as c_int,
        )
    };
    let n = (n.max(0) as usize).min(buf.len());
    let mut out = Vec::new();
    let mut info = vec![0u8; VNODE_FDINFO_WITH_PATH];
    // struct proc_fdinfo { int32_t proc_fd; uint32_t proc_fdtype; }
    for rec in buf[..n].as_chunks::<8>().0 {
        let fd = i32::from_ne_bytes(rec[0..4].try_into().unwrap());
        let kind = u32::from_ne_bytes(rec[4..8].try_into().unwrap());
        if kind != libc::PROX_FDTYPE_VNODE as u32 {
            continue;
        }
        let got = unsafe {
            libc::proc_pidfdinfo(
                pid,
                fd,
                PROC_PIDFDVNODEPATHINFO,
                info.as_mut_ptr().cast(),
                info.len() as c_int,
            )
        };
        if got as usize != VNODE_FDINFO_WITH_PATH {
            continue;
        }
        let flags = u32::from_ne_bytes(info[FI_OPENFLAGS..FI_OPENFLAGS + 4].try_into().unwrap());
        let vtype = i32::from_ne_bytes(info[VI_TYPE..VI_TYPE + 4].try_into().unwrap());
        if flags & FWRITE == 0 || vtype != VREG {
            continue;
        }
        let path = &info[VIP_PATH..];
        let len = path.iter().position(|&b| b == 0).unwrap_or(path.len());
        if len > 0 {
            use std::os::unix::ffi::OsStrExt;
            out.push(std::ffi::OsStr::from_bytes(&path[..len]).into());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn rusage(pid: i32) -> Option<libc::rusage_info_v4> {
    let mut ri = MaybeUninit::<libc::rusage_info_v4>::uninit();
    let rc = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, ri.as_mut_ptr().cast()) };
    (rc == 0).then(|| unsafe { ri.assume_init() })
}

fn all_pids() -> Vec<i32> {
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if n <= 0 {
        return Vec::new();
    }
    // Room for processes started between the two calls.
    let mut pids = vec![0i32; n as usize + 128];
    let bytes = (pids.len() * size_of::<i32>()) as c_int;
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast::<c_void>(), bytes) };
    pids.truncate(n.max(0) as usize);
    pids.retain(|&p| p > 0);
    pids
}

/// An app, by its bundle on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub pid: i32,
    /// The bundle's name, "Safari" for Safari.app.
    pub name: String,
    pub bundle: std::path::PathBuf,
}

/// The app `pid` works for: the process macOS holds responsible for it
/// (Activity Monitor's account), which for an app's helper, such as
/// WebKit's networking process, is the app that started it. `None` when
/// that isn't an app.
pub fn responsible_app(pid: i32) -> Option<App> {
    let r = responsible(pid).unwrap_or(pid);
    let bundle = outermost_app(&path(r)?)?;
    Some(App {
        pid: r,
        name: bundle.file_stem()?.to_string_lossy().into_owned(),
        bundle,
    })
}

/// Private, but what Activity Monitor uses; looked up when needed, so a
/// macOS without it only loses the app's name.
fn responsible(pid: i32) -> Option<i32> {
    type Responsible = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;
    static FOUND: std::sync::OnceLock<Option<Responsible>> = std::sync::OnceLock::new();
    let f = (*FOUND.get_or_init(|| {
        let sym = unsafe {
            libc::dlsym(
                libc::RTLD_DEFAULT,
                c"responsibility_get_pid_responsible_for_pid".as_ptr(),
            )
        };
        (!sym.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, Responsible>(sym) })
    }))?;
    let r = unsafe { f(pid) };
    (r > 0).then_some(r)
}

/// Asks `pid` to end (SIGTERM), as Activity Monitor's Quit does to a
/// process without windows.
pub fn terminate(pid: i32) -> std::io::Result<()> {
    if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Ends `pid` at once (SIGKILL), as Activity Monitor's Force Quit does: for
/// a process that doesn't end when asked.
pub fn force_quit(pid: i32) -> std::io::Result<()> {
    if unsafe { libc::kill(pid, libc::SIGKILL) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Whether `pid` is still running.
pub fn alive(pid: i32) -> bool {
    let found = unsafe { libc::kill(pid, 0) } == 0;
    found || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// The executable `pid` runs.
pub fn path(pid: i32) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    Some(std::ffi::OsString::from_vec(buf).into())
}

/// The outermost `.app` an executable is in: a helper app inside an app's
/// bundle belongs to that app.
fn outermost_app(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut at = std::path::PathBuf::new();
    for c in exe.components() {
        at.push(c);
        if at.extension().is_some_and(|e| e == "app") {
            return Some(at);
        }
    }
    None
}

fn owner(pid: i32) -> Option<u32> {
    let mut info = MaybeUninit::<libc::proc_bsdshortinfo>::uninit();
    let size = size_of::<libc::proc_bsdshortinfo>() as c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            PROC_PIDT_SHORTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    (n == size).then(|| unsafe { info.assume_init() }.pbsi_uid)
}

fn disk_io(pid: i32) -> Option<(u64, u64)> {
    rusage(pid).map(|ri| (ri.ri_diskio_bytesread, ri.ri_diskio_byteswritten))
}

fn name(pid: i32) -> String {
    let mut buf = [0u8; 256];
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return "?".into();
    }
    String::from_utf8_lossy(&buf[..n as usize]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sees_itself_and_its_writes() {
        let me = std::process::id() as i32;
        let before = own_processes()
            .into_iter()
            .find(|p| p.pid == me)
            .expect("own process listed");
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("w.bin");
        std::fs::write(&f, vec![7u8; 8 << 20]).unwrap();
        std::fs::File::open(&f).unwrap().sync_all().unwrap();
        let after = own_processes().into_iter().find(|p| p.pid == me).unwrap();
        assert!(after.bytes_written >= before.bytes_written);
        assert!(!after.name.is_empty());
    }

    #[test]
    fn an_apps_helper_belongs_to_the_outermost_app() {
        let p = std::path::Path::new(
            "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/\
             Helpers/Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper",
        );
        assert_eq!(
            outermost_app(p).as_deref(),
            Some(std::path::Path::new("/Applications/Google Chrome.app"))
        );
        assert_eq!(
            outermost_app(std::path::Path::new("/usr/libexec/cfprefsd")),
            None
        );
        // Some process is responsible for this one (a terminal, say).
        let me = std::process::id() as i32;
        assert!(responsible(me).is_some_and(|r| r > 0));
        assert!(path(me).is_some_and(|p| p.is_absolute()));
    }

    #[test]
    fn lists_files_open_for_writing_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let (w, r) = (root.join("written.log"), root.join("read.txt"));
        std::fs::write(&r, b"x").unwrap();
        let _w = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&w)
            .unwrap();
        let _r = std::fs::File::open(&r).unwrap();
        let open = files_open_for_writing(std::process::id() as i32);
        eprintln!("{open:?}");
        assert!(open.contains(&w), "{open:?}");
        assert!(!open.contains(&r), "{open:?}");
    }
}
