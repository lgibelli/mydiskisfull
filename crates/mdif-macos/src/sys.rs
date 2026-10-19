//! System-wide facts: when the Mac last started, how much swap it holds, and
//! whether this process may look everywhere.

use std::ffi::CString;
use std::fs::File;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// Files only a process with Full Disk Access can open, which refuse quietly
/// rather than prompt: the privacy databases. macOS 27 has no per-user one,
/// so the system's comes first.
pub fn access_probes(home: &Path) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Library/Application Support/com.apple.TCC/TCC.db"),
        home.join("Library/Application Support/com.apple.TCC/TCC.db"),
    ]
}

/// Whether this process has Full Disk Access: it can open one of `probes`
/// ([`access_probes`]), or is refused one (`EPERM`). Asking never shows a
/// prompt. `None` when none of them exists.
pub fn full_disk_access(probes: &[PathBuf]) -> Option<bool> {
    let mut refused = false;
    for p in probes {
        match File::open(p) {
            Ok(_) => return Some(true),
            Err(e) if e.raw_os_error() == Some(libc::EPERM) => refused = true,
            Err(_) => {}
        }
    }
    refused.then_some(false)
}

/// Send this process's standard error to `file`. A LaunchAgent inside an app
/// can't name a log in the user's home folder, so the watcher opens its own.
pub fn redirect_stderr(file: &File) -> io::Result<()> {
    // SAFETY: both are open descriptors; dup2 replaces fd 2 atomically.
    if unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Unix time the Mac started (`kern.boottime`). A setting that takes effect
/// "after a restart" is in effect once this is later than the change.
pub fn boot_time() -> io::Result<i64> {
    // SAFETY: `timeval` is plain data, and `kern.boottime` is one timeval.
    let tv: libc::timeval = unsafe { sysctl("kern.boottime")? };
    Ok(tv.tv_sec)
}

/// Bytes of swap in use (`vm.swapusage`). Swap files live on the VM volume,
/// which shares the APFS container with the data, so they take free space
/// until the Mac restarts.
pub fn swap_used() -> io::Result<u64> {
    // SAFETY: `xsw_usage` is plain data, and `vm.swapusage` is one of them.
    let x: libc::xsw_usage = unsafe { sysctl("vm.swapusage")? };
    Ok(x.xsu_used)
}

/// # Safety
/// `T` must be plain data, and exactly what the sysctl `name` returns.
unsafe fn sysctl<T>(name: &str) -> io::Result<T> {
    let c = CString::new(name)?;
    let mut v = MaybeUninit::<T>::uninit();
    let mut len = size_of::<T>();
    let rc = unsafe {
        libc::sysctlbyname(
            c.as_ptr(),
            v.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    if len != size_of::<T>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{name}: {len} bytes, expected {}", size_of::<T>()),
        ));
    }
    Ok(unsafe { v.assume_init() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn booted_in_the_past() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let boot = boot_time().unwrap();
        // After 2020-01-01, and before now.
        assert!(boot > 1_577_836_800 && boot < now, "{boot}");
    }

    #[test]
    fn full_disk_access_is_any_probe_that_opens() {
        let tmp = tempfile::tempdir().unwrap();
        let (gone, there) = (tmp.path().join("gone"), tmp.path().join("there"));
        assert_eq!(full_disk_access(std::slice::from_ref(&gone)), None);
        File::create(&there).unwrap();
        assert_eq!(full_disk_access(&[gone, there]), Some(true));
        // The real probes exist on every Mac, so the answer is known.
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
        assert!(full_disk_access(&access_probes(&home)).is_some());
    }

    #[test]
    fn stderr_goes_to_the_file() {
        use std::io::{Read, Write};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("log");
        let file = File::create(&path).unwrap();
        // SAFETY: duplicating and restoring fd 2 around the redirect.
        let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
        assert!(saved >= 0);
        redirect_stderr(&file).unwrap();
        io::stderr().write_all(b"to the log\n").unwrap();
        unsafe { libc::dup2(saved, libc::STDERR_FILENO) };
        unsafe { libc::close(saved) };
        let mut text = String::new();
        File::open(&path)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        // Other tests may write to stderr meanwhile; ours must be there.
        assert!(text.contains("to the log\n"), "{text:?}");
    }

    #[test]
    fn swap_matches_sysctl() {
        let used = swap_used().unwrap();
        let out = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "vm.swapusage"])
            .output()
            .unwrap();
        // "total = 12288.00M  used = 10596.12M  free = ..."
        let text = String::from_utf8_lossy(&out.stdout);
        let mb: f64 = text
            .split("used = ")
            .nth(1)
            .and_then(|r| r.split('M').next())
            .and_then(|n| n.trim().parse().ok())
            .unwrap();
        let theirs = (mb * 1024.0 * 1024.0) as u64;
        // Swap moves between the two reads; 256 MB of slack.
        assert!(used.abs_diff(theirs) < 256 << 20, "{used} vs {theirs}");
    }
}
