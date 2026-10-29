//! Volume statistics, process-wide file-system policy, and the Trash.

use std::ffi::{CStr, CString, OsStr};
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use libc::c_int;

// <sys/resource.h>
const IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES: c_int = 3;
const IOPOL_SCOPE_PROCESS: c_int = 0;
const IOPOL_MATERIALIZE_DATALESS_FILES_OFF: c_int = 1;

unsafe extern "C" {
    fn setiopolicy_np(iotype: c_int, scope: c_int, policy: c_int) -> c_int;
    // <sys/fsgetpath.h>
    fn fsgetpath(buf: *mut libc::c_char, size: usize, fsid: *mut libc::fsid_t, id: u64) -> isize;
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

/// Mounted file systems, as (device, mount point): `("/dev/disk3s5",
/// "/System/Volumes/Data")`.
pub fn mounts() -> io::Result<Vec<(String, String)>> {
    let n = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    // Room for a few mounted since the count.
    let mut all: Vec<libc::statfs> = Vec::with_capacity(n as usize + 8);
    let size = (all.capacity() * size_of::<libc::statfs>()) as c_int;
    let got = unsafe { libc::getfsstat(all.as_mut_ptr(), size, libc::MNT_NOWAIT) };
    if got < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: getfsstat filled `got` entries, no more than the capacity.
    unsafe { all.set_len(got as usize) };
    let text = |chars: &[libc::c_char]| {
        unsafe { CStr::from_ptr(chars.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    };
    Ok(all
        .iter()
        .map(|s| (text(&s.f_mntfromname), text(&s.f_mntonname)))
        .collect())
}

/// Bytes the volume mounted at `mount` uses by itself (`ATTR_VOL_SPACEUSED`),
/// what `diskutil` calls its capacity in use.
pub fn space_used(mount: &Path) -> io::Result<u64> {
    // The returned length, then the attribute, packed to 4 bytes.
    #[repr(C, packed(4))]
    struct Buf {
        len: u32,
        used: libc::off_t,
    }
    let c = CString::new(mount.as_os_str().as_bytes())?;
    let mut list: libc::attrlist = unsafe { std::mem::zeroed() };
    list.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    list.volattr = libc::ATTR_VOL_INFO | libc::ATTR_VOL_SPACEUSED;
    let mut buf = MaybeUninit::<Buf>::zeroed();
    let rc = unsafe {
        libc::getattrlist(
            c.as_ptr(),
            (&raw mut list).cast(),
            buf.as_mut_ptr().cast(),
            size_of::<Buf>(),
            0,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let buf = unsafe { buf.assume_init() };
    let (len, used) = (buf.len, buf.used);
    if (len as usize) < size_of::<Buf>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: no space used", mount.display()),
        ));
    }
    Ok(used as u64)
}

/// Device id of `path` itself (not following a final symlink).
/// A volume's file-system ID, to find what's on it by number ([`path_of`]).
#[derive(Clone, Copy)]
pub struct FsId(libc::fsid_t);

impl std::fmt::Debug for FsId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FsId")
    }
}

/// The ID of the volume holding `path`.
pub fn fsid(path: &Path) -> io::Result<FsId> {
    let c = CString::new(path.as_os_str().as_bytes())?;
    let mut st = MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::statfs(c.as_ptr(), st.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(FsId(unsafe { st.assume_init() }.f_fsid))
}

/// Where the file or folder with inode number `inode` on the volume is now:
/// it follows renames and moves (to the Trash too), and is `NotFound` once
/// it's deleted. Paths on the boot volume come as FSEvents reports them,
/// through its firmlinks (`/Users`, not `/System/Volumes/Data/Users`).
pub fn path_of(fsid: FsId, inode: u64) -> io::Result<PathBuf> {
    let mut buf = vec![0u8; 4 * libc::PATH_MAX as usize];
    let mut id = fsid.0;
    // SAFETY: the buffer is as long as said, and the call writes a
    // NUL-terminated path within it.
    let n = unsafe { fsgetpath(buf.as_mut_ptr().cast(), buf.len(), &mut id, inode) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Ok(PathBuf::from(OsStr::from_bytes(&buf[..end])))
}

pub fn dev_of(path: &Path) -> io::Result<i32> {
    use std::os::unix::fs::MetadataExt;
    Ok(std::fs::symlink_metadata(path)?.dev() as i32)
}

/// Moves `path` to the Trash of its volume, as Finder does, and returns
/// where it went. Its space comes back once the Trash is emptied.
pub fn trash(path: &Path) -> io::Result<std::path::PathBuf> {
    use objc2::rc::Retained;
    use objc2_foundation::{NSFileManager, NSString, NSURL};

    let text = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a UTF-8 path"))?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(text));
    let mut moved: Option<Retained<NSURL>> = None;
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, Some(&mut moved))
        .map_err(|e| io::Error::other(e.localizedDescription().to_string()))?;
    moved
        .and_then(|u| u.path())
        .map(|p| std::path::PathBuf::from(p.to_string()))
        .ok_or_else(|| io::Error::other("the Trash didn't say where it went"))
}

/// What Finder calls available on the volume holding `path`: the free space
/// plus what macOS would delete by itself to make room for something
/// important (purgeable caches, local snapshots, cloud files it can download
/// again). `NSURLVolumeAvailableCapacityForImportantUsageKey`.
pub fn available_for_important_usage(path: &Path) -> io::Result<u64> {
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::AnyObject;
    use objc2_foundation::{
        NSNumber, NSString, NSURL, NSURLVolumeAvailableCapacityForImportantUsageKey,
    };

    let text = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a UTF-8 path"))?;
    autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(text));
        let mut value: Option<Retained<AnyObject>> = None;
        // SAFETY: the key is a constant Foundation exports, and its value is
        // an NSNumber, checked below before it's used.
        let key = unsafe { NSURLVolumeAvailableCapacityForImportantUsageKey };
        unsafe { url.getResourceValue_forKey_error(&mut value, key) }
            .map_err(|e| io::Error::other(e.localizedDescription().to_string()))?;
        let n = value
            .and_then(|v| v.downcast::<NSNumber>().ok())
            .ok_or_else(|| io::Error::other("no capacity for important usage"))?;
        Ok(n.longLongValue().max(0) as u64)
    })
}

/// Keeps `path` out of Time Machine's backups, or lets it back in, as
/// `tmutil addexclusion` does without `-p`: a mark on the item itself
/// (`NSURLIsExcludedFromBackupKey`), which goes where it's moved, and goes
/// with it when it's deleted. No administrator needed for your own files.
pub fn set_excluded_from_backup(path: &Path, excluded: bool) -> io::Result<()> {
    use objc2::rc::autoreleasepool;
    use objc2_foundation::{NSNumber, NSString, NSURL, NSURLIsExcludedFromBackupKey};

    let text = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a UTF-8 path"))?;
    autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(text));
        let value = NSNumber::new_bool(excluded);
        // SAFETY: the key is a constant Foundation exports, and its value
        // is the NSNumber it documents.
        let key = unsafe { NSURLIsExcludedFromBackupKey };
        unsafe { url.setResourceValue_forKey_error(Some(&value), key) }
            .map_err(|e| io::Error::other(e.localizedDescription().to_string()))
    })
}

/// Whether `path` itself carries that mark. Time Machine also leaves out
/// what its settings list and macOS's own exclusions (caches, the Trash):
/// `tmutil isexcluded` says all of it.
pub fn excluded_from_backup(path: &Path) -> io::Result<bool> {
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSNumber, NSString, NSURL, NSURLIsExcludedFromBackupKey};

    let text = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a UTF-8 path"))?;
    autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(text));
        let mut value: Option<Retained<AnyObject>> = None;
        // SAFETY: as above; the value is checked to be an NSNumber.
        let key = unsafe { NSURLIsExcludedFromBackupKey };
        unsafe { url.getResourceValue_forKey_error(&mut value, key) }
            .map_err(|e| io::Error::other(e.localizedDescription().to_string()))?;
        Ok(value
            .and_then(|v| v.downcast::<NSNumber>().ok())
            .is_some_and(|n| n.boolValue()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_kept_out_of_backups_says_so_and_comes_back_in() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("target");
        std::fs::create_dir(&d).unwrap();
        assert!(!excluded_from_backup(&d).unwrap());
        set_excluded_from_backup(&d, true).unwrap();
        assert!(excluded_from_backup(&d).unwrap());
        // The mark Cargo puts on its target folders, which tmutil reads.
        let names = xattr_names(&d);
        assert!(
            names
                .iter()
                .any(|n| n.ends_with("com_apple_backup_excludeItem")),
            "{names:?}"
        );
        set_excluded_from_backup(&d, false).unwrap();
        assert!(!excluded_from_backup(&d).unwrap());
    }

    fn xattr_names(p: &Path) -> Vec<String> {
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
        let mut buf = vec![0u8; 4096];
        let n = unsafe { libc::listxattr(c.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        assert!(n >= 0);
        buf[..n as usize]
            .split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    }

    #[test]
    fn a_folder_is_found_by_number_after_it_moves_and_not_once_deleted() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let a = root.join("a");
        std::fs::create_dir(&a).unwrap();
        let inode = std::fs::metadata(&a).unwrap().ino();
        let id = fsid(&root).unwrap();
        assert_eq!(path_of(id, inode).unwrap(), a);
        let b = root.join("moved").join("b");
        std::fs::create_dir(root.join("moved")).unwrap();
        std::fs::rename(&a, &b).unwrap();
        assert_eq!(path_of(id, inode).unwrap(), b);
        std::fs::remove_dir(&b).unwrap();
        let e = path_of(id, inode).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound, "{e}");
    }

    #[test]
    fn available_for_important_usage_counts_at_least_the_free_space() {
        let data = Path::new("/System/Volumes/Data");
        let important = available_for_important_usage(data).unwrap();
        let st = statfs(data).unwrap();
        // Free space moves while the test runs: allow 1 GB of slack.
        assert!(
            important + 1_000_000_000 >= st.avail && important <= st.total,
            "{important} vs {st:?}"
        );
        assert!(available_for_important_usage(Path::new("/no/such/place")).is_err());
    }

    /// On a disk image of its own, so the real Trash stays as it was: an
    /// item goes to that volume's `.Trashes`, and the image is thrown away.
    #[test]
    fn trash_moves_an_item_to_its_volumes_trash() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("trash.dmg");
        let mount = dir.path().join("mnt");
        let hdiutil = |args: &[&std::ffi::OsStr]| {
            let ok = Command::new("hdiutil")
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "hdiutil {args:?}");
        };
        hdiutil(&[
            "create".as_ref(),
            "-quiet".as_ref(),
            "-size".as_ref(),
            "20m".as_ref(),
            "-fs".as_ref(),
            "APFS".as_ref(),
            "-volname".as_ref(),
            "mdif-trash".as_ref(),
            image.as_os_str(),
        ]);
        std::fs::create_dir(&mount).unwrap();
        hdiutil(&[
            "attach".as_ref(),
            "-quiet".as_ref(),
            "-nobrowse".as_ref(),
            "-owners".as_ref(),
            "on".as_ref(),
            "-mountpoint".as_ref(),
            mount.as_os_str(),
            image.as_os_str(),
        ]);
        struct Detach<'a>(&'a Path);
        impl Drop for Detach<'_> {
            fn drop(&mut self) {
                let _ = std::process::Command::new("hdiutil")
                    .args(["detach", "-quiet", "-force"])
                    .arg(self.0)
                    .status();
            }
        }
        let _detach = Detach(&mount);

        let item = mount.join("model");
        std::fs::create_dir(&item).unwrap();
        std::fs::write(item.join("weights.bin"), vec![1u8; 4096]).unwrap();
        let moved = trash(&item).unwrap();
        assert!(!item.exists());
        assert!(moved.join("weights.bin").is_file(), "{}", moved.display());
        assert!(
            moved.starts_with(mount.join(".Trashes"))
                || moved.to_string_lossy().contains("/.Trashes/"),
            "{}",
            moved.display()
        );
        assert!(trash(&mount.join("gone")).is_err());
    }
    #[test]
    fn statfs_of_root_volume() {
        let st = statfs(Path::new("/")).unwrap();
        assert!(st.total > 0 && st.avail <= st.free && st.free <= st.total);
        assert!(st.device.starts_with("/dev/disk"), "{}", st.device);
    }

    #[test]
    fn data_volume_is_mounted_and_uses_space() {
        let data = statfs(Path::new("/System/Volumes/Data")).unwrap();
        let mounts = mounts().unwrap();
        assert!(
            mounts
                .iter()
                .any(|(d, m)| *d == data.device && *m == data.mount_point),
            "{mounts:?}"
        );
        let used = space_used(Path::new(&data.mount_point)).unwrap();
        assert!(used > 0 && used <= data.total, "{used}");
    }

    #[test]
    fn materialization_can_be_disabled() {
        forbid_dataless_materialization().unwrap();
    }
}
