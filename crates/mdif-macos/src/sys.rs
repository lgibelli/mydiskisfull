//! System-wide facts: when the Mac last started, and how much swap it holds.

use std::ffi::CString;
use std::io;
use std::mem::MaybeUninit;

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
