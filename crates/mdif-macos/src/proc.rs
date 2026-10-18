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
}
