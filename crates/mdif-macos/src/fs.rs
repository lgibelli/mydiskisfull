//! Volume statistics and process-wide file-system policy.

use std::ffi::{CStr, CString};
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use libc::c_int;

// <sys/resource.h>
const IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES: c_int = 3;
const IOPOL_SCOPE_PROCESS: c_int = 0;
const IOPOL_MATERIALIZE_DATALESS_FILES_OFF: c_int = 1;

unsafe extern "C" {
    fn setiopolicy_np(iotype: c_int, scope: c_int, policy: c_int) -> c_int;
}

/// Stop this process from downloading cloud placeholders (OneDrive, iCloud,
/// ...) when it touches them. Scanning must never cost network traffic:
/// listing a dataless directory would otherwise fetch it.
pub fn forbid_dataless_materialization() -> io::Result<()> {
    let rc = unsafe {
        setiopolicy_np(
            IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
            IOPOL_SCOPE_PROCESS,
            IOPOL_MATERIALIZE_DATALESS_FILES_OFF,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct FsStats {
    /// Bytes in the volume's container (APFS volumes share their container).
    pub total: u64,
    pub free: u64,
    /// Free for unprivileged users; what Finder and `df` report.
    pub avail: u64,
    /// e.g. `/dev/disk3s5`.
    pub device: String,
    /// e.g. `/System/Volumes/Data`.
    pub mount_point: String,
}

pub fn statfs(path: &Path) -> io::Result<FsStats> {
    let c = CString::new(path.as_os_str().as_bytes())?;
    let mut st = MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::statfs(c.as_ptr(), st.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let st = unsafe { st.assume_init() };
    let bs = st.f_bsize as u64;
    let text = |chars: &[libc::c_char]| {
        unsafe { CStr::from_ptr(chars.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    };
    Ok(FsStats {
        total: st.f_blocks * bs,
        free: st.f_bfree * bs,
        avail: st.f_bavail * bs,
        device: text(&st.f_mntfromname),
        mount_point: text(&st.f_mntonname),
    })
}

/// Device id of `path` itself (not following a final symlink).
pub fn dev_of(path: &Path) -> io::Result<i32> {
    use std::os::unix::fs::MetadataExt;
    Ok(std::fs::symlink_metadata(path)?.dev() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statfs_of_root_volume() {
        let st = statfs(Path::new("/")).unwrap();
        assert!(st.total > 0 && st.avail <= st.free && st.free <= st.total);
        assert!(st.device.starts_with("/dev/disk"), "{}", st.device);
    }

    #[test]
    fn materialization_can_be_disabled() {
        forbid_dataless_materialization().unwrap();
    }
}
