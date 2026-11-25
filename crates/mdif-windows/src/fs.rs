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
    GetDiskFreeSpaceExW, GetDriveTypeW, GetFileInformationByHandle, GetFileInformationByHandleEx,
    GetFinalPathNameByHandleW, GetLogicalDriveStringsW, GetVolumeInformationW,
    GetVolumeNameForVolumeMountPointW, GetVolumePathNameW, LOCKFILE_EXCLUSIVE_LOCK, LockFileEx,
    OpenFileById,
};
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::CreateEventW;

pub use crate::private::{make_private, private_to_me};
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
    // Windows gives `total` and `avail` for the calling user (with disk
    // quotas, what the quota allows) and `free` for the whole volume: only
    // the two for the user add up, so those are what's reported.
    let Space { avail, total, .. } = space_of(&mount)?;
    Ok(FsStats {
        total,
        free: avail,
        avail,
        device: volume_name(&mount),
        mount_point: text(&mount),
    })
}

/// Whether the figures `statfs` gives are the caller's quota and not the
/// volume's: Windows then gives this user less free space than the volume has.
pub fn quota_limited(path: &Path) -> io::Result<bool> {
    let space = space_of(&volume_path(&std::path::absolute(path)?)?)?;
    Ok(space.avail != space.free)
}

/// A volume's space as `GetDiskFreeSpaceExW` gives it.
struct Space {
    /// What this user may still write: the volume's free space, or less
    /// with a disk quota.
    avail: u64,
    /// What this user may have on it: its size, or the quota.
    total: u64,
    /// What's free on the volume, for everyone.
    free: u64,
}

/// The space of the volume mounted at `mount` (NUL-ended).
fn space_of(mount: &[u16]) -> io::Result<Space> {
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: `mount` is NUL-ended; the outputs are valid for writes.
    if unsafe { GetDiskFreeSpaceExW(mount.as_ptr(), &mut avail, &mut total, &mut free) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Space { avail, total, free })
}

/// Whether the volume holding `path` is a network drive or share: its server
/// reports the volume behind it, not the folder shared.
pub fn is_remote(path: &Path) -> io::Result<bool> {
    let mount = volume_path(&std::path::absolute(path)?)?;
    // SAFETY: `mount` is NUL-ended.
    let kind = unsafe { GetDriveTypeW(mount.as_ptr()) };
    drive_is_remote(kind, &text(&mount))
}

/// [`is_remote`] from the drive type Windows gave and the volume's root path.
/// DRIVE_REMOTE is 4. A UNC root, ordinary or verbatim (`\\?\UNC\s\x`), is a
/// share whatever the type says. 0 and 1 (unknown, no root) say nothing about
/// the rest: not "local".
fn drive_is_remote(kind: u32, root: &str) -> io::Result<bool> {
    if kind == 4
        || root.starts_with(r"\\?\UNC\")
        || (root.starts_with(r"\\") && !root.starts_with(r"\\?\"))
    {
        return Ok(true);
    }
    if kind <= 1 {
        return Err(io::Error::other("the drive type can't be told"));
    }
    Ok(false)
}

/// The name of the file system of the volume holding `path`: `NTFS`, `ReFS`,
/// `exFAT`, `FAT32`.
pub fn file_system(path: &Path) -> io::Result<String> {
    let mount = volume_path(&std::path::absolute(path)?)?;
    let mut name = [0u16; 64];
    // SAFETY: `mount` is NUL-ended; `name` is the length passed; the other
    // outputs are left out.
    let ok = unsafe {
        GetVolumeInformationW(
            mount.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(text(&name))
}

/// Whether the volume holding `path` records when a file was last opened
/// (read), so "not opened in months" can be told there. Only NTFS is
/// trusted, and NTFS records it only when `NtfsDisableLastAccessUpdate`
/// says so: since Windows 10 1803 the system decides at start-up and leaves
/// it off on a system drive over 128 GB, so on most PCs it isn't.
pub fn records_last_access(path: &Path) -> io::Result<bool> {
    if !file_system(path)?.eq_ignore_ascii_case("NTFS") {
        return Ok(false);
    }
    // No value (or one that isn't a number): not recorded, as before 1803.
    let policy = crate::registry::machine_number(
        r"SYSTEM\CurrentControlSet\Control\FileSystem",
        "NtfsDisableLastAccessUpdate",
    )
    .unwrap_or(None);
    Ok(policy.is_some_and(last_access_on))
}

/// Whether `NtfsDisableLastAccessUpdate`'s value leaves NTFS updating when
/// a file was last opened: 0 (as before Windows 10 1803), 0x80000000 (set
/// by the user) or 0x80000002 (left to the system, which turned it on).
/// Anything else (1, 0x80000001, 0x80000003: off) isn't.
fn last_access_on(policy: u32) -> bool {
    matches!(policy, 0 | 0x8000_0000 | 0x8000_0002)
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
    let (serial, root) = volume_serial(path)?;
    // Remembered, so a number on this volume can be turned back into a place.
    let mut known = VOLUMES.lock().unwrap_or_else(|e| e.into_inner());
    match known.iter_mut().find(|(s, _)| *s == serial) {
        Some(entry) => entry.1 = root,
        None => known.push((serial, root)),
    }
    Ok(FsId(serial))
}

/// The serial number of the volume holding `path`, and the path of its root: by
/// its own name where it has one, so a volume mounted in a folder is the volume
/// and not the folder it is mounted in.
fn volume_serial(path: &Path) -> io::Result<(u32, PathBuf)> {
    let stats = statfs(path)?;
    let root = PathBuf::from(if stats.device.starts_with(r"\\?\Volume{") {
        stats.device
    } else {
        stats.mount_point
    });
    let serial = by_handle(&open_with(&root, FILE_READ_ATTRIBUTES)?)?.dwVolumeSerialNumber;
    Ok((serial, root))
}

/// Device id of the volume holding `path` (what `dev_of` of any file on it says).
pub fn dev_of_volume(path: &Path) -> io::Result<i32> {
    Ok(volume_serial(path)?.0 as i32)
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

/// Moves `path` to its drive's Recycle Bin, as Explorer's Delete does, and
/// says where it went there. Never deleted for good: what Windows would
/// delete at once is refused and left as it is. Told here, before the shell
/// is asked: a drive or share as a whole, a volume with no bin (removable,
/// network, a share, one with no letter or mounted in a folder), a bin set
/// to delete at once, and a file bigger than the bin takes, which the shell
/// deletes for good while telling the sink it recycles it (seen on Windows
/// 11). The sink refuses the rest (`shell.rs`): a folder too big for the bin
/// goes item by item, each marked to be deleted for good, as another
/// project found (CI checks that one is never deleted for good). Only files
/// go to the bin on Windows so far (`clean::unused_file`).
pub fn trash(path: &Path) -> io::Result<PathBuf> {
    let path = plain(std::path::absolute(path)?);
    if is_root(&path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a drive or a share as a whole doesn't go to the Recycle Bin",
        ));
    }
    let mount = volume_path(&path)?;
    // SAFETY: `mount` is NUL-ended.
    let kind = unsafe { GetDriveTypeW(mount.as_ptr()) };
    if !has_recycle_bin(kind, &text(&mount)) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this drive has no Recycle Bin (a removable or network drive, a share, a volume \
             with no letter or mounted in a folder): Windows would delete it for good, so it's \
             left as it is",
        ));
    }
    let meta = std::fs::symlink_metadata(&path)?;
    match bin_takes(&bin_settings(&mount)?, space_of(&mount)?.total) {
        None => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Windows is set to delete files on this drive at once, not to move them to the \
                 Recycle Bin (in the bin's Properties, or by Group Policy): it's left as it is",
            ));
        }
        Some(most) if meta.is_file() && !fits(meta.len(), most) => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "it's bigger than this drive's Recycle Bin takes ({} MB, as its Properties \
                     count them): Windows would delete it for good, so it's left as it is",
                    most >> 20
                ),
            ));
        }
        Some(_) => {}
    }
    crate::shell::recycle(&path)
}

/// Whether `path` is a drive, a share or a volume as a whole (`C:\`,
/// `\\server\share`): nothing to move anywhere.
fn is_root(path: &Path) -> bool {
    path.parent().is_none()
}

/// Whether what's on the volume mounted at `mount` (`GetVolumePathNameW`'s
/// answer), a drive of type `kind` (`GetDriveTypeW`'s), can go to a Recycle
/// Bin: a fixed drive's (3), mounted at its letter (`C:\`). Removable (2)
/// and network (4) drives, discs and RAM disks have none, a share neither,
/// and the shell doesn't recycle from a volume named only by its GUID or
/// mounted in a folder.
fn has_recycle_bin(kind: u32, mount: &str) -> bool {
    let b = mount.as_bytes();
    let lettered =
        b.len() == 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    kind == 3 && lettered
}

/// How a volume's Recycle Bin is set: in the bin's Properties (per volume,
/// under `BitBucket\Volume\{GUID}`) and by Group Policy.
#[derive(Debug, Default)]
struct BinSettings {
    /// "Don't move files to the Recycle Bin" (`NukeOnDelete`), or the policy
    /// "Do not move deleted files to the Recycle Bin" (`NoRecycleFiles`).
    deletes_at_once: bool,
    /// The bin's maximum size, in MB as Windows counts them (MiB):
    /// `MaxCapacity`, there once the bin has been used or set.
    max_mb: Option<u32>,
    /// The policy "Maximum allowed Recycle Bin size": a percentage of the
    /// user's quota on the volume (`RecycleBinSize`).
    policy_percent: Option<u32>,
}

/// The most a file may be and still go to the bin set as `s`, on a volume
/// where the user may have `quota` bytes; `None` when nothing goes there.
/// With nothing set, Windows' default: a tenth of the first 40 GB of the
/// quota and a twentieth of the rest. With more than one set, the least.
fn bin_takes(s: &BinSettings, quota: u64) -> Option<u64> {
    const FIRST: u64 = 40_000_000_000;
    if s.deletes_at_once {
        return None;
    }
    let set = [
        s.max_mb.map(|mb| u64::from(mb) << 20),
        s.policy_percent
            .map(|p| quota / 100 * u64::from(p.min(100))),
    ];
    let default = quota.min(FIRST) / 10 + quota.saturating_sub(FIRST) / 20;
    Some(set.into_iter().flatten().min().unwrap_or(default))
}

/// Whether a file of `len` bytes fits a bin that takes `most`, with a
/// mebibyte to spare for how Windows rounds what it counts.
fn fits(len: u64, most: u64) -> bool {
    len.checked_add(1 << 20).is_some_and(|n| n <= most)
}

/// How the Recycle Bin of the volume mounted at `mount` is set (see
/// [`BinSettings`]). A volume with no name to file its bin's settings under
/// is an error: what they say can't be told.
fn bin_settings(mount: &[u16]) -> io::Result<BinSettings> {
    use crate::registry::{machine_number, user_number};
    const POLICIES: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies";
    let mut s = BinSettings::default();
    let explorer = format!(r"{POLICIES}\Explorer");
    // Documented under Explorer's policies and Network's: either.
    for key in [explorer.clone(), format!(r"{POLICIES}\Network")] {
        for n in [
            user_number(&key, "NoRecycleFiles")?,
            machine_number(&key, "NoRecycleFiles")?,
        ] {
            s.deletes_at_once |= n.is_some_and(|n| n != 0);
        }
    }
    s.policy_percent = [
        user_number(&explorer, "RecycleBinSize")?,
        machine_number(&explorer, "RecycleBinSize")?,
    ]
    .into_iter()
    .flatten()
    .min();
    let name = volume_name(mount);
    let guid = volume_guid(&name).ok_or_else(|| {
        io::Error::other(
            "this drive's volume has no name its Recycle Bin's settings go by: it's left as it is",
        )
    })?;
    let key =
        format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{guid}");
    s.deletes_at_once |= user_number(&key, "NukeOnDelete")?.is_some_and(|n| n != 0);
    s.max_mb = user_number(&key, "MaxCapacity")?;
    Ok(s)
}

/// The `{…}` of a volume's name (`\\?\Volume{…}\`): what its bin's
/// settings are filed under.
fn volume_guid(name: &str) -> Option<&str> {
    let start = name.find('{')?;
    let end = start + name[start..].find('}')?;
    Some(&name[start..=end])
}

/// An `HRESULT` failure as an error: Windows' own error where it carries
/// one (`0x8007xxxx`), its number otherwise.
pub(crate) fn hresult(hr: i32) -> io::Error {
    let code = hr as u32;
    if code & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((code & 0xFFFF) as i32)
    } else {
        io::Error::other(format!("the shell answered 0x{code:08X}"))
    }
}

/// How many items the Recycle Bin of the drive whose root is `root` (`C:\`)
/// holds for this user, as the shell counts them from the bin's index:
/// nothing in it is walked.
pub fn recycle_bin_items(root: &Path) -> io::Result<u64> {
    let n = query_recycle_bin(&wide(root.as_os_str()))?.i64NumItems;
    Ok(n.max(0) as u64)
}

fn query_recycle_bin(root: &[u16]) -> io::Result<windows_sys::Win32::UI::Shell::SHQUERYRBINFO> {
    use windows_sys::Win32::UI::Shell::{SHQUERYRBINFO, SHQueryRecycleBinW};
    let mut info = SHQUERYRBINFO {
        cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `root` is NUL-ended and `info` is sized as the call reads it.
    let hr = unsafe { SHQueryRecycleBinW(root.as_ptr(), &mut info) };
    if hr < 0 {
        return Err(hresult(hr));
    }
    Ok(info)
}

/// Empties the Recycle Bin of the drive whose root is `root` (`C:\`), for
/// this user: what's in it is erased, as Explorer's Empty Recycle Bin does,
/// with no question (the app asked first), no progress window and no sound.
pub fn empty_recycle_bin(root: &Path) -> io::Result<()> {
    use windows_sys::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::UI::Shell::{
        SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND, SHEmptyRecycleBinW,
    };
    let root = wide(root.as_os_str());
    // An empty bin is a failure to the call: nothing to do.
    if query_recycle_bin(&root)?.i64NumItems == 0 {
        return Ok(());
    }
    // The shell may reach the bin through COM: this thread joins it for the
    // call, unless it already has (in another mode, which is left as it is).
    // SAFETY: plain calls; each successful initialisation is undone below.
    let com = unsafe {
        CoInitializeEx(
            std::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    if com < 0 && com != RPC_E_CHANGED_MODE {
        return Err(hresult(com));
    }
    // SAFETY: `root` is NUL-ended; no window is passed, so none is shown.
    let hr = unsafe {
        SHEmptyRecycleBinW(
            std::ptr::null_mut(),
            root.as_ptr(),
            SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND,
        )
    };
    if com >= 0 {
        // SAFETY: pairs the successful `CoInitializeEx` above.
        unsafe { CoUninitialize() };
    }
    if hr < 0 {
        return Err(hresult(hr));
    }
    Ok(())
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
    let path = final_path_of_handle(h);
    // SAFETY: the handle is ours, and closed once.
    unsafe { CloseHandle(h) };
    path.map(|p| Some(plain(p)))
}

/// The path an open handle names, as the file system spells it: with a drive
/// letter when the volume has one, and otherwise by the volume's own name
/// (`\\?\Volume{...}\x`), which is how a volume mounted in a folder is told.
fn final_path_of_handle(h: HANDLE) -> io::Result<PathBuf> {
    // VOLUME_NAME_DOS, then VOLUME_NAME_GUID where the volume has no letter.
    for (flags, last) in [(0u32, false), (1u32, true)] {
        let mut buf = vec![0u16; 1024];
        loop {
            // SAFETY: the handle is open; `buf` is as long as passed.
            let n =
                unsafe { GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, flags) }
                    as usize;
            if n == 0 {
                let err = io::Error::last_os_error();
                // Path not found: no DOS name for this volume.
                if !last && err.raw_os_error() == Some(3) {
                    break;
                }
                return Err(err);
            }
            if n < buf.len() {
                return Ok(PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])));
            }
            buf.resize(n + 1, 0);
        }
    }
    unreachable!("the second spelling returns or fails")
}

/// `path` as the journal and [`folder_by_id`] spell paths (the same drive letter
/// or volume name, the real case, a mounted volume reached through its folder):
/// so a watched root and the folders found in it can be compared. As given if it
/// can't be opened.
pub fn normalized(path: &Path) -> PathBuf {
    // Followed, unlike every other open here: a mount point in the path is the
    // volume that is mounted.
    let file = OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_NO_RECALL)
        .open(path);
    file.and_then(|f| final_path_of_handle(f.as_raw_handle() as HANDLE))
        .map_or_else(|_| path.to_path_buf(), plain)
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
mod recycle_tests {
    use super::*;

    #[test]
    fn last_access_counts_only_where_ntfs_is_told_to_record_it() {
        for on in [0, 0x8000_0000, 0x8000_0002] {
            assert!(last_access_on(on), "{on:#x}");
        }
        for off in [1, 0x8000_0001, 0x8000_0003, 2, 3, 0xffff_ffff] {
            assert!(!last_access_on(off), "{off:#x}");
        }
        // This PC's, whatever it is: a yes or a no, never an error.
        let here = std::env::temp_dir();
        println!("records last access: {:?}", records_last_access(&here));
        assert!(records_last_access(&here).is_ok());
    }

    #[test]
    fn only_a_fixed_drive_mounted_at_its_letter_has_a_recycle_bin() {
        assert!(has_recycle_bin(3, r"C:\"));
        assert!(has_recycle_bin(3, "D:/"));
        for (kind, mount) in [
            (2, r"E:\"),
            (4, r"Z:\"),
            (5, r"F:\"),
            (6, r"R:\"),
            (0, r"C:\"),
            (1, r"C:\"),
            (3, r"\\server\share\"),
            (3, r"\\?\Volume{0a1b2c3d-0000-0000-0000-100000000000}\"),
            // A volume mounted in a folder of another.
            (3, r"C:\mnt\backup\"),
            (3, r"relative\"),
            (3, "C:"),
        ] {
            assert!(!has_recycle_bin(kind, mount), "{kind} {mount}");
        }
    }

    #[test]
    fn a_drive_or_a_share_as_a_whole_isnt_moved() {
        for root in [
            r"C:\",
            r"\\server\share",
            r"\\server\share\",
            r"\\?\Volume{0a1b2c3d-0000-0000-0000-100000000000}\",
        ] {
            assert!(is_root(Path::new(root)), "{root}");
        }
        for item in [r"C:\Users", r"C:\big.iso", r"\\server\share\x"] {
            assert!(!is_root(Path::new(item)), "{item}");
        }
    }

    #[test]
    fn the_bin_takes_what_it_is_set_to_or_windows_default() {
        const GB: u64 = 1_000_000_000;
        let none = BinSettings::default();
        // A tenth of the first 40 GB, a twentieth of the rest.
        assert_eq!(bin_takes(&none, 30 * GB), Some(3 * GB));
        assert_eq!(bin_takes(&none, 500 * GB), Some(4 * GB + 23 * GB));
        assert_eq!(bin_takes(&none, 0), Some(0));
        // Its Properties, in MiB.
        let set = BinSettings {
            max_mb: Some(18_685),
            ..Default::default()
        };
        assert_eq!(bin_takes(&set, 500 * GB), Some(18_685 << 20));
        // A policy's percentage of the quota, and the least of the two.
        let policy = BinSettings {
            max_mb: Some(18_685),
            policy_percent: Some(1),
            ..Default::default()
        };
        assert_eq!(bin_takes(&policy, 500 * GB), Some(5 * GB));
        let over = BinSettings {
            policy_percent: Some(250),
            ..Default::default()
        };
        assert_eq!(bin_takes(&over, 500 * GB), Some(500 * GB));
        // Set to delete at once: nothing goes.
        let at_once = BinSettings {
            deletes_at_once: true,
            max_mb: Some(18_685),
            ..Default::default()
        };
        assert_eq!(bin_takes(&at_once, 500 * GB), None);
    }

    #[test]
    fn a_file_fits_the_bin_with_a_mebibyte_to_spare() {
        const MB: u64 = 1 << 20;
        assert!(fits(MB, 2 * MB));
        assert!(fits(0, MB));
        assert!(!fits(MB + 1, 2 * MB));
        assert!(!fits(8 * MB, 4 * MB));
        assert!(!fits(0, 0));
        assert!(!fits(u64::MAX, u64::MAX));
    }

    #[test]
    fn a_volumes_bin_settings_go_by_its_guid() {
        assert_eq!(
            volume_guid(r"\\?\Volume{4c1b02c1-d990-11dc-99ae-806e6f6e6963}\"),
            Some("{4c1b02c1-d990-11dc-99ae-806e6f6e6963}")
        );
        assert_eq!(volume_guid(r"C:\"), None);
        assert_eq!(volume_guid(""), None);
        assert_eq!(volume_guid("{unended"), None);
        // This PC's own drive has one, and its bin's settings read.
        let here = volume_path(&std::env::temp_dir()).unwrap();
        println!("this drive's bin: {:?}", bin_settings(&here));
        assert!(volume_guid(&volume_name(&here)).is_some());
        assert!(bin_settings(&here).is_ok());
    }

    /// The tests that use this user's Recycle Bin for real, one at a time:
    /// one changes how it's set.
    static BIN: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// For real, on CI's runner (`cargo test -p mdif-windows -- --ignored
    /// recycle`): a file and a folder go to the runner's Recycle Bin and are
    /// found there; what Windows would delete for good (a file reached
    /// through a share) is refused, by the drive check and by the shell's
    /// own answer, and stays.
    #[test]
    #[ignore = "moves files to this user's Recycle Bin: CI runs it"]
    fn recycle_moves_to_the_bin_and_never_deletes_for_good() {
        let _one = BIN.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let in_bin = |to: &Path| {
            let to = to.to_string_lossy().to_ascii_lowercase();
            println!("in the bin: {to}");
            to.get(1..)
                .is_some_and(|t| t.starts_with(r":\$recycle.bin"))
        };
        // The drive's bin, as its own index says what it holds.
        let root = {
            let p = plain(std::path::absolute(dir.path()).unwrap());
            PathBuf::from(format!(r"{}\", &p.to_string_lossy()[..2]))
        };
        let before = recycle_bin_items(&root).unwrap();
        let file = dir.path().join("mdif-recycle-probe.txt");
        std::fs::write(&file, b"probe").unwrap();
        let to = trash(&file).unwrap();
        assert!(!file.exists(), "still where it was");
        assert!(to.exists() && in_bin(&to), "{}", to.display());
        let after = recycle_bin_items(&root).unwrap();
        println!(
            "the bin of {}: {before} items, then {after}",
            root.display()
        );
        assert!(after > before, "{before} then {after}");
        let folder = dir.path().join("mdif-recycle-folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("inside.txt"), b"x").unwrap();
        let to = trash(&folder).unwrap();
        assert!(
            !folder.exists() && to.is_dir() && in_bin(&to),
            "{}",
            to.display()
        );
        assert!(trash(&dir.path().join("not-there")).is_err());
        // The same file through the drive's administrative share: no bin.
        let kept = dir.path().join("mdif-recycle-kept.txt");
        std::fs::write(&kept, b"keep me").unwrap();
        let local = plain(std::path::absolute(&kept).unwrap());
        let local = local.to_string_lossy();
        let share = PathBuf::from(format!(r"\\localhost\{}${}", &local[..1], &local[2..]));
        if share.exists() {
            assert!(trash(&share).is_err());
            let asked = crate::shell::recycle(&share);
            println!("the shell, asked anyway: {asked:?}");
            assert!(asked.is_err());
            assert!(kept.exists(), "a share's file was deleted for good");
        } else {
            println!("no administrative share here: the refusal through one isn't checked");
        }
    }

    /// For real, on CI's runner (`--ignored recycle`): with this drive's bin
    /// set to take 4 MB, a file of 8 MB is refused and stays, and one of 1 MB
    /// goes; with the bin set to delete at once, nothing goes. How the bin
    /// was set is put back, even when an assertion fails.
    #[test]
    #[ignore = "changes how this user's Recycle Bin is set, for a moment: CI runs it"]
    fn recycle_refuses_what_the_bin_would_delete_for_good() {
        use crate::registry::{set_user_number, user_number};
        let _one = BIN.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let mount = volume_path(&plain(std::path::absolute(dir.path()).unwrap())).unwrap();
        let name = volume_name(&mount);
        let key = format!(
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{}",
            volume_guid(&name).unwrap()
        );
        struct PutBack(String, Option<u32>, Option<u32>);
        impl Drop for PutBack {
            fn drop(&mut self) {
                let _ = set_user_number(&self.0, "MaxCapacity", self.1);
                let _ = set_user_number(&self.0, "NukeOnDelete", self.2);
            }
        }
        let before = (
            user_number(&key, "MaxCapacity").unwrap(),
            user_number(&key, "NukeOnDelete").unwrap(),
        );
        println!("{key}: MaxCapacity, NukeOnDelete {before:?}");
        let was = PutBack(key.clone(), before.0, before.1);
        set_user_number(&key, "MaxCapacity", Some(4)).unwrap();
        set_user_number(&key, "NukeOnDelete", Some(0)).unwrap();

        let big = dir.path().join("mdif-too-big.bin");
        std::fs::write(&big, vec![7u8; 8 << 20]).unwrap();
        let refused = trash(&big).unwrap_err();
        println!("8 MB, the bin taking 4: {refused}");
        assert!(refused.to_string().contains("bigger than"), "{refused}");
        assert!(big.exists(), "a file too big for the bin was deleted");

        let small = dir.path().join("mdif-fits.bin");
        std::fs::write(&small, vec![7u8; 1 << 20]).unwrap();
        let to = trash(&small).unwrap();
        assert!(!small.exists() && to.exists(), "{}", to.display());

        // A folder too big for the bin is left to the shell and the sink:
        // refused and left whole, or moved whole (a shell that read the
        // bin's size before it was set), never deleted for good.
        let folder = dir.path().join("mdif-too-big-folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("inside.bin"), vec![7u8; 8 << 20]).unwrap();
        match trash(&folder) {
            Ok(to) => {
                println!(
                    "8 MB in a folder, the bin taking 4: moved whole, to {}",
                    to.display()
                );
                assert!(!folder.exists(), "still where it was");
                // Where the shell said it went, or the bin when it didn't.
                assert!(
                    to.join("inside.bin").exists() || to.ends_with("$Recycle.Bin"),
                    "{}",
                    to.display()
                );
            }
            Err(e) => {
                println!("8 MB in a folder, the bin taking 4: {e}");
                assert!(
                    folder.join("inside.bin").exists(),
                    "a folder was deleted for good"
                );
            }
        }

        // The shell, asked anyway: what the check is there for. (Recycled,
        // refused or deleted for good, it's this test's own file.)
        let asked = crate::shell::recycle(&big);
        println!(
            "the shell, asked anyway for 8 MB: {asked:?}; still where it was: {}",
            big.exists()
        );

        set_user_number(&key, "NukeOnDelete", Some(1)).unwrap();
        let at_once = dir.path().join("mdif-at-once.bin");
        std::fs::write(&at_once, b"x").unwrap();
        let refused = trash(&at_once).unwrap_err();
        println!("the bin set to delete at once: {refused}");
        assert!(
            at_once.exists(),
            "deleted for good by a bin set to delete at once"
        );
        drop(was);
        let after = (
            user_number(&key, "MaxCapacity").unwrap(),
            user_number(&key, "NukeOnDelete").unwrap(),
        );
        assert_eq!(after, before, "the bin isn't set as it was");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drive_is_remote_by_its_type_or_its_root() {
        // DRIVE_FIXED (3) and DRIVE_REMOVABLE (2) are local.
        assert!(!drive_is_remote(3, r"C:\").unwrap());
        assert!(!drive_is_remote(2, r"E:\").unwrap());
        assert!(!drive_is_remote(3, r"\\?\Volume{1}\").unwrap());
        // DRIVE_REMOTE, and a share by its root even when the type isn't told.
        assert!(drive_is_remote(4, r"Z:\").unwrap());
        assert!(drive_is_remote(0, r"\\server\share\").unwrap());
        assert!(drive_is_remote(1, r"\\?\UNC\server\share\").unwrap());
        // Unclassified and not a share: unknown, not local.
        assert!(drive_is_remote(0, r"Z:\").is_err());
        assert!(drive_is_remote(1, r"Z:\").is_err());
    }

    #[test]
    fn the_file_system_is_named_and_the_quota_is_told() {
        let dir = tempfile::tempdir().unwrap();
        let kind = file_system(dir.path()).unwrap();
        assert!(!kind.is_empty());
        // Whatever the account's quota, the probes answer; a temp folder is local.
        quota_limited(dir.path()).unwrap();
        // (Where a temp folder is depends on the environment; the classification is
        // tested apart.)
        let _ = is_remote(dir.path());
    }

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
            set_excluded_from_backup(p, true).unwrap_err(),
            excluded_from_backup(p).unwrap_err(),
            has_mark(p, "x").unwrap_err(),
        ] {
            assert_eq!(e.kind(), io::ErrorKind::Unsupported, "{e}");
        }
    }
}
