//! Volumes and files: free space, identity, locks.
//!
//! Every handle opened here asks for attributes only (never data), never
//! follows a reparse point, and never recalls a cloud placeholder, so
//! looking at a file never changes it, its access time, or what's on disk.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use std::ptr::null;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_IO_PENDING, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_DESCRIPTOR, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FileAttributeTagInfo, FileStandardInfo,
    GetDiskFreeSpaceExW, GetFileInformationByHandle, GetFileInformationByHandleEx,
    GetFinalPathNameByHandleW, GetLogicalDriveStringsW, GetVolumeNameForVolumeMountPointW,
    GetVolumePathNameW, LOCKFILE_EXCLUSIVE_LOCK, LockFileEx, OpenFileById,
};
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::CreateEventW;

use crate::{unsupported, wide};

/// `FILE_FLAG_OPEN_NO_RECALL`: opening a cloud placeholder doesn't download it.
const FILE_FLAG_OPEN_NO_RECALL: u32 = 0x0010_0000;

/// There is no process-wide switch on Windows: every handle this crate opens
/// carries `FILE_FLAG_OPEN_NO_RECALL` instead. So this only says so.
pub fn forbid_dataless_materialization() -> io::Result<()> {
    Ok(())
}

#[derive(Debug, Clone)]
pub struct FsStats {
    /// Bytes on the volume, or what this user's disk quota allows.
    pub total: u64,
    /// The same as `avail`: the only free space that adds up with `total`.
    pub free: u64,
    /// Free for this user.
    pub avail: u64,
    /// The volume's own name, e.g. `\\?\Volume{GUID}\`.
    pub device: String,
    /// Where it's mounted, e.g. `C:\`.
    pub mount_point: String,
}

impl FsStats {
    /// What the volume's history is kept under: the volume's own name (its GUID),
    /// which stays with the disk, so two removable disks that take turns as `E:\`
    /// don't share a forecast, and a disk that changes letter keeps its own.
    pub fn history_key(&self) -> String {
        if self.device.starts_with(r"\\?\Volume{") {
            self.device.clone()
        } else {
            self.mount_point.clone()
        }
    }
}

/// The mount point of the volume holding `path`: `C:\`.
fn volume_path(path: &Path) -> io::Result<Vec<u16>> {
    let p = wide(path.as_os_str());
    // Room for a long path: the answer is never longer than the question.
    let mut buf = vec![0u16; p.len().max(260) + 1];
    // SAFETY: `p` is NUL-ended and `buf` is the length passed.
    if unsafe { GetVolumePathNameW(p.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    buf.truncate(n + 1);
    Ok(buf)
}

fn text(wide: &[u16]) -> String {
    let n = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..n])
}

/// The volume's own name for the mount point `mount` (NUL-ended).
fn volume_name(mount: &[u16]) -> String {
    let mut name = [0u16; 64];
    // SAFETY: `mount` is NUL-ended and `name` is the length passed.
    let ok = unsafe {
        GetVolumeNameForVolumeMountPointW(mount.as_ptr(), name.as_mut_ptr(), name.len() as u32)
    };
    if ok == 0 { String::new() } else { text(&name) }
}

pub fn statfs(path: &Path) -> io::Result<FsStats> {
    // The macOS call fails for what isn't there; so does this.
    std::fs::symlink_metadata(path)?;
    // A path without a drive is on the current drive, which the lookup
    // can't know without being told where that is.
    let mount = volume_path(&std::path::absolute(path)?)?;
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: `mount` is NUL-ended; the outputs are valid for writes.
    if unsafe { GetDiskFreeSpaceExW(mount.as_ptr(), &mut avail, &mut total, &mut free) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // Windows gives `total` and `avail` for the calling user (with disk
    // quotas, what the quota allows) and `free` for the whole volume: only
    // the two for the user add up, so those are what's reported.
    let _ = free;
    Ok(FsStats {
        total,
        free: avail,
        avail,
        device: volume_name(&mount),
        mount_point: text(&mount),
    })
}

/// Mounted file systems, as (volume name, mount point): `(r"\\?\Volume{...}\",
/// r"C:\")`. Drives with a letter only.
pub fn mounts() -> io::Result<Vec<(String, String)>> {
    let mut buf = vec![0u16; 512];
    // SAFETY: `buf` is the length passed.
    let n = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) } as usize;
    if n == 0 {
        return Err(io::Error::last_os_error());
    }
    if n > buf.len() {
        return Err(io::Error::other("too many drives"));
    }
    Ok(buf[..n]
        .split(|&c| c == 0)
        .filter(|d| !d.is_empty())
        .map(|d| {
            let mut mount = d.to_vec();
            mount.push(0);
            (volume_name(&mount), text(&mount))
        })
        .collect())
}

/// Bytes the volume mounted at `mount` uses.
pub fn space_used(mount: &Path) -> io::Result<u64> {
    let s = statfs(mount)?;
    Ok(s.total.saturating_sub(s.free))
}

/// A volume's serial number, to find what's on it by number.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FsId(u32);

/// What the file system says about one file or folder, from a handle opened
/// for attributes only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileInfo {
    /// The volume's serial number.
    pub volume: u32,
    /// The file's number on its volume: the same for every hard link.
    pub index: u64,
    /// How many names it has.
    pub links: u32,
    /// Bytes it takes on the disk.
    pub allocated: u64,
    /// Its length, as the file system has it now.
    pub logical: u64,
}

/// A handle on `path` for attributes only, not following a link at `path`
/// itself and not recalling a placeholder.
pub(crate) fn open_attributes(path: &Path) -> io::Result<File> {
    open_with(path, FILE_READ_ATTRIBUTES)
}

/// [`open_attributes`] with more access: listing a folder needs
/// `FILE_LIST_DIRECTORY` as well.
pub(crate) fn open_with(path: &Path, access: u32) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(access)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL,
        )
        .open(path)
}

pub(crate) fn by_handle(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    // SAFETY: zeroed is a valid value of this plain struct; the handle is
    // the open file's, for as long as `file` is.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info)
}

/// Identity, link count and allocated size of `path`, which is not followed
/// if it is a link.
pub fn file_info(path: &Path) -> io::Result<FileInfo> {
    let file = open_attributes(path)?;
    let info = by_handle(&file)?;
    // SAFETY: zeroed is a valid value of this plain struct, and its size is
    // the one passed.
    let mut std_info: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as HANDLE,
            FileStandardInfo,
            (&raw mut std_info).cast(),
            size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(FileInfo {
        volume: info.dwVolumeSerialNumber,
        index: u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow),
        links: info.nNumberOfLinks,
        allocated: std_info.AllocationSize.max(0) as u64,
        logical: std_info.EndOfFile.max(0) as u64,
    })
}

/// The attributes and the reparse tag of what the handle is open on (tag 0 if
/// it has none): what kind of reparse point, which tells a link (a name
/// surrogate) from a cloud file or a compressed one, which are still the file
/// they look like. The attributes here can name a recall flag that
/// `BY_HANDLE_FILE_INFORMATION` leaves out.
pub(crate) fn attributes_and_tag(file: &File) -> io::Result<(u32, u32)> {
    // SAFETY: zeroed is a valid value of this plain struct, and its size is
    // the one passed; the handle is the open file's.
    let mut info: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as HANDLE,
            FileAttributeTagInfo,
            (&raw mut info).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((info.FileAttributes, info.ReparseTag))
}

/// Whether a reparse tag names another place (a symbolic link, a junction, a
/// mount point): the name-surrogate bit.
pub(crate) fn is_link_tag(tag: u32) -> bool {
    tag & 0x2000_0000 != 0
}

/// How many names the file at `path` has, if it is the file numbered `index`
/// on the volume `volume` (what a listing saw: the path may have been
/// replaced since): one open for attributes and one query, about 13 µs.
/// Links aren't followed.
pub fn link_count_of(path: &Path, volume: u32, index: u64) -> io::Result<u32> {
    let info = by_handle(&open_attributes(path)?)?;
    let found = u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow);
    if info.dwVolumeSerialNumber != volume || found != index {
        return Err(io::Error::other("it is not the file that was listed"));
    }
    Ok(info.nNumberOfLinks)
}

/// The ID of the volume holding `path`.
pub fn fsid(path: &Path) -> io::Result<FsId> {
    let serial = by_handle(&open_attributes(path)?)?.dwVolumeSerialNumber;
    // Remembered, so a number on this volume can be turned back into a place.
    if let Ok(stats) = statfs(path) {
        let root = if stats.device.starts_with(r"\\?\Volume{") {
            stats.device
        } else {
            stats.mount_point
        };
        let mut known = VOLUMES.lock().unwrap_or_else(|e| e.into_inner());
        match known.iter_mut().find(|(s, _)| *s == serial) {
            Some(entry) => entry.1 = PathBuf::from(root),
            None => known.push((serial, PathBuf::from(root))),
        }
    }
    Ok(FsId(serial))
}

/// Where the folder numbered `inode` on the volume is now, if it is: found by
/// its number through the volume's root, which is known from an earlier
/// [`fsid`] of a path on it.
pub fn path_of(fsid: FsId, inode: u64) -> io::Result<PathBuf> {
    let root = VOLUMES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(serial, _)| *serial == fsid.0)
        .map(|(_, root)| root.clone())
        .ok_or_else(|| io::Error::other("no path on that volume was asked its identity"))?;
    let volume = open_with(&root, 0)?;
    folder_by_id(volume.as_raw_handle() as HANDLE, inode)?
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
}

/// The roots of the volumes whose identity was asked, by serial number: how a
/// number is turned back into a place ([`path_of`]).
static VOLUMES: std::sync::Mutex<Vec<(u32, PathBuf)>> = std::sync::Mutex::new(Vec::new());

/// Device id of `path` itself (not following a final link): its volume's
/// serial number, as the same bits a macOS device number would have.
pub fn dev_of(path: &Path) -> io::Result<i32> {
    Ok(by_handle(&open_attributes(path)?)?.dwVolumeSerialNumber as i32)
}

/// Moves `path` to the Recycle Bin. Not written yet.
pub fn trash(_path: &Path) -> io::Result<PathBuf> {
    Err(unsupported("the Recycle Bin"))
}

/// What the user can use on the volume holding `path`. Windows has nothing
/// it frees by itself, so it is the free space.
pub fn available_for_important_usage(path: &Path) -> io::Result<u64> {
    Ok(statfs(path)?.avail)
}

/// Backup exclusion is a Time Machine idea. Not on Windows.
pub fn set_excluded_from_backup(_path: &Path, _excluded: bool) -> io::Result<()> {
    Err(unsupported("excluding from backups"))
}

pub fn excluded_from_backup(_path: &Path) -> io::Result<bool> {
    Err(unsupported("excluding from backups"))
}

/// A named mark on a file (an extended attribute on macOS). Not written yet.
pub fn set_mark(_path: &Path, _name: &str, _on: bool) -> io::Result<()> {
    Err(unsupported("marking a file"))
}

pub fn has_mark(_path: &Path, _name: &str) -> io::Result<bool> {
    Err(unsupported("marking a file"))
}

/// Waits for, then holds, `file`'s lock until it's closed: one process at a
/// time does what it guards.
pub fn lock(file: &File) -> io::Result<()> {
    // SAFETY: zeroed is a valid OVERLAPPED (offset 0); the handle is the
    // open file's, for as long as `file` is.
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    // Its own event: a handle opened for overlapped I/O answers at once and
    // finishes later, and is waited for on this, not on the file's.
    // SAFETY: plain call; the event is closed below on every path.
    let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        return Err(io::Error::last_os_error());
    }
    overlapped.hEvent = event;
    let result = lock_with(file, &mut overlapped);
    // SAFETY: the event is ours, and closed once, after the request ended.
    unsafe { CloseHandle(event) };
    result
}

fn lock_with(file: &File, overlapped: &mut OVERLAPPED) -> io::Result<()> {
    // SAFETY: the handle is the open file's; `overlapped` outlives the
    // request, which is waited for before this returns.
    let ok = unsafe {
        LockFileEx(
            file.as_raw_handle() as HANDLE,
            LOCKFILE_EXCLUSIVE_LOCK,
            0,
            u32::MAX,
            u32::MAX,
            overlapped,
        )
    };
    if ok != 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
        return Err(err);
    }
    let mut done = 0u32;
    // SAFETY: the same handle and the same OVERLAPPED the request was made with.
    let waited =
        unsafe { GetOverlappedResult(file.as_raw_handle() as HANDLE, overlapped, &mut done, 1) };
    if waited == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// The folder numbered `id` on the volume `volume` is a handle on, as a plain path; `Ok(None)` once
/// it isn't there any more (its removal is a change of the folder that held
/// it, which has its own record). Any other failure is an error: a change
/// that couldn't be placed.
pub(crate) fn folder_by_id(volume: HANDLE, id: u64) -> io::Result<Option<PathBuf>> {
    let desc = FILE_ID_DESCRIPTOR {
        dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32,
        Type: 0, // FileIdType: the 64-bit number
        Anonymous: windows_sys::Win32::Storage::FileSystem::FILE_ID_DESCRIPTOR_0 {
            FileId: id as i64,
        },
    };
    // SAFETY: `desc` is the descriptor the call wants; the handle is checked.
    let h = unsafe {
        OpenFileById(
            volume,
            &desc,
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_NO_RECALL,
        )
    };
    if h.is_null() || h as isize == -1 {
        let err = io::Error::last_os_error();
        // File not found, path not found, invalid parameter (a number
        // that names nothing any more): gone.
        return match err.raw_os_error() {
            Some(2 | 3 | 87) => Ok(None),
            _ => Err(err),
        };
    }
    let mut buf = vec![0u16; 1024];
    let path = loop {
        // SAFETY: the handle is open; `buf` is as long as passed.
        let n =
            unsafe { GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, 0) } as usize;
        if n == 0 {
            break Err(io::Error::last_os_error());
        }
        if n < buf.len() {
            break Ok(std::ffi::OsString::from_wide(&buf[..n]));
        }
        buf.resize(n + 1, 0);
    };
    // SAFETY: the handle is ours, and closed once.
    unsafe { CloseHandle(h) };
    path.map(|p| Some(plain(PathBuf::from(p))))
}

/// `\\?\C:\x` as `C:\x`, and `\\?\UNC\s\x` as `\\s\x`: the paths the rest of the
/// program uses. A volume with no drive letter is only named by its GUID
/// (`\\?\Volume{...}\x`), which stays as it is: without the prefix it would be a
/// relative path.
pub(crate) fn plain(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\")
        && rest.as_bytes().get(1) == Some(&b':')
    {
        PathBuf::from(rest)
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moved_folder_is_found_again_by_its_number() {
        let dir = tempfile::tempdir().unwrap();
        let root = plain(std::fs::canonicalize(dir.path()).unwrap());
        std::fs::create_dir_all(root.join("a/inner")).unwrap();
        let id = |parent: &Path, name: &str| {
            crate::bulk::read_dir(parent)
                .unwrap()
                .into_iter()
                .find(|e| e.name == name)
                .unwrap()
                .inode
        };
        let volume = fsid(&root).unwrap();
        let number = id(&root, "a");
        assert_eq!(path_of(volume, number).unwrap(), root.join("a"));
        // Renamed and moved: it is where it went, with what's in it.
        std::fs::create_dir(root.join("b")).unwrap();
        std::fs::rename(root.join("a"), root.join("b/moved")).unwrap();
        assert_eq!(path_of(volume, number).unwrap(), root.join("b/moved"));
        assert_eq!(
            path_of(volume, id(&root.join("b/moved"), "inner")).unwrap(),
            root.join("b/moved/inner")
        );
        // Deleted: gone, as a not-found error.
        std::fs::remove_dir_all(root.join("b/moved")).unwrap();
        assert_eq!(
            path_of(volume, number).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        // A volume nobody asked about can't be turned into a place.
        assert!(path_of(FsId(1), number).is_err());
    }

    #[test]
    fn a_folders_volume_is_told_by_its_space_and_its_number() {
        let dir = tempfile::tempdir().unwrap();
        let s = statfs(dir.path()).unwrap();
        assert!(
            s.total > 0 && s.avail == s.free && s.free <= s.total,
            "{s:?}"
        );
        assert!(s.mount_point.ends_with('\\'), "{s:?}");
        assert!(s.device.starts_with(r"\\?\Volume{"), "{s:?}");
        // History follows the disk, not its letter.
        assert_eq!(s.history_key(), s.device);
        let lettered = FsStats {
            device: String::new(),
            ..s.clone()
        };
        assert_eq!(lettered.history_key(), s.mount_point);
        let file = dir.path().join("f");
        std::fs::write(&file, "x").unwrap();
        // The folder and a file in it are on one volume.
        assert_eq!(dev_of(dir.path()).unwrap(), dev_of(&file).unwrap());
        assert_eq!(fsid(dir.path()).unwrap(), fsid(&file).unwrap());
        assert!(space_used(Path::new(&s.mount_point)).unwrap() > 0);
        assert!(
            mounts()
                .unwrap()
                .iter()
                .any(|(_, mount)| *mount == s.mount_point)
        );
        assert!(statfs(&dir.path().join("missing")).is_err());
        // A path without a drive is looked up where it is, not on the boot
        // drive (where the tests run from the same drive anyway).
        let here = statfs(Path::new(".")).unwrap();
        let there = statfs(&std::env::current_dir().unwrap()).unwrap();
        assert_eq!(here.mount_point, there.mount_point);
    }

    #[test]
    fn hard_links_are_one_file_and_a_files_size_is_what_it_takes() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::write(&a, vec![1u8; 10_000]).unwrap();
        std::fs::hard_link(&a, &b).unwrap();
        let (ia, ib) = (file_info(&a).unwrap(), file_info(&b).unwrap());
        assert_eq!(ia.links, 2);
        assert_eq!((ia.volume, ia.index), (ib.volume, ib.index));
        assert_ne!(ia.index, 0);
        assert!(ia.allocated >= 10_000 && ia.allocated % 512 == 0, "{ia:?}");
    }

    #[test]
    fn a_link_is_looked_at_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, vec![1u8; 100_000]).unwrap();
        let link = dir.path().join("link");
        // Symbolic links need a privilege; a junction doesn't, but needs
        // a folder. Skip where neither can be made.
        if std::os::windows::fs::symlink_file(&target, &link).is_err() {
            return;
        }
        assert!(file_info(&link).unwrap().allocated < 100_000);
    }

    #[test]
    fn one_process_at_a_time_holds_a_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");
        let a = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        lock(&a).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let b = OpenOptions::new().write(true).open(p).unwrap();
            lock(&b).unwrap();
            tx.send(()).unwrap();
        });
        // Blocked while `a` holds it...
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(300))
                .is_err()
        );
        drop(a);
        // ...and let through once it's closed.
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn what_isnt_written_yet_says_so() {
        let p = Path::new(r"C:\");
        for e in [
            trash(p).unwrap_err(),
            set_excluded_from_backup(p, true).unwrap_err(),
            excluded_from_backup(p).unwrap_err(),
            has_mark(p, "x").unwrap_err(),
        ] {
            assert_eq!(e.kind(), io::ErrorKind::Unsupported, "{e}");
        }
    }
}
