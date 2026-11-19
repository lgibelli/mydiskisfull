//! Listing a directory: each entry's kind, sizes, times and attributes.
//!
//! One open handle, checked and listed: it is opened for listing without
//! following a link or recalling a placeholder, what it turned out to be is
//! read from the handle itself (a link, a placeholder or a file is refused),
//! and the listing comes from that same handle (`FileIdExtdDirectoryInfo`),
//! so what was checked is what is listed. No file in it is opened, and no
//! access time changes. Allocated size and file number come with each entry.
//!
//! The listing has no link count. Files of [`LINK_MIN`] bytes or more are
//! asked (one open each) so a big file with two names is counted once; a
//! smaller one is taken to have one name. NTFS has no clones, so private size
//! is allocated size and nothing shares blocks.
//!
//! A file held open for writing is listed at the size it had when last
//! closed or flushed: a growing log is seen late (measured: docs/WINDOWS.md).

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::Path;

use windows_sys::Win32::Foundation::{ERROR_NO_MORE_FILES, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ID_EXTD_DIR_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
    FileIdExtdDirectoryInfo, FileIdExtdDirectoryRestartInfo, GetFileInformationByHandleEx,
};

use crate::fs;

/// `FILE_ATTRIBUTE_*` of cloud files, sparse files and links.
const OFFLINE: u32 = 0x0000_1000;
const RECALL_ON_OPEN: u32 = 0x0004_0000;
const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
const SPARSE_FILE: u32 = 0x0000_0200;
const REPARSE_POINT: u32 = 0x0000_0400;

/// Files this big, in allocated bytes, have their link count asked of the file
/// system; smaller ones are taken to have one name. One whose count can't be
/// had (it isn't ours to open, or it changed since the listing) is [`LINKS_UNKNOWN`]:
/// possibly shared, which never says more is freed than would be.
pub const LINK_MIN: u64 = 1 << 20;

/// `nlink` of a file whose count couldn't be had: more names than any scan will
/// find, so it's deduplicated but never taken to be wholly ours.
pub const LINKS_UNKNOWN: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: OsString,
    pub kind: Kind,
    /// The volume the entry lives on (its serial number, as the same bits a
    /// macOS device number would have).
    pub dev: i32,
    /// Creation, modification and last access time, seconds since the
    /// epoch. Reading them reads no file, so it changes no access time.
    pub crtime: i64,
    pub mtime: i64,
    pub atime: i64,
    /// `FILE_ATTRIBUTE_*`.
    pub flags: u32,
    /// The file's number on its volume (0 until it is read).
    pub inode: u64,
    /// Names the file has (1 until it is read).
    pub nlink: u32,
    pub logical: u64,
    pub allocated: u64,
    /// Bytes freed immediately if this file were deleted.
    pub private: u64,
    /// Same value for files that are pure clones of each other: none.
    pub clone_id: u64,
    pub ext_flags: u64,
    /// Number of full clones sharing all of this file's blocks: none.
    pub clone_refcnt: u32,
    /// Per-entry error (0 = none).
    pub error: u32,
}

impl Entry {
    /// Cloud placeholder: listed, but its content is not on this disk.
    pub fn is_dataless(&self) -> bool {
        self.flags & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0
    }

    /// Allocated bytes shared with a clone: none.
    pub fn shared(&self) -> u64 {
        self.allocated.saturating_sub(self.private)
    }

    /// NTFS has no clones.
    pub fn may_share_blocks(&self) -> bool {
        false
    }

    pub fn is_purgeable(&self) -> bool {
        false
    }

    pub fn is_sparse(&self) -> bool {
        self.flags & SPARSE_FILE != 0
    }
}

/// Open `dir` for listing, not following a link at `dir` itself and not
/// recalling it, and check from the handle that it is a folder that may be
/// listed: not a link, not a cloud placeholder (whose listing would be
/// downloaded), not a file.
fn open_listing(dir: &Path) -> io::Result<File> {
    let refuse = |why: &str| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{}: {why}", dir.display()),
        )
    };
    let file = fs::open_with(dir, FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY)?;
    // From both queries: a recall flag may be in only one of them.
    let (tagged, tag) = fs::attributes_and_tag(&file)?;
    let attributes = fs::by_handle(&file)?.dwFileAttributes | tagged;
    // A reparse point is a link only if it names another place; a folder a
    // cloud provider has fully downloaded keeps its reparse tag and is a folder.
    if attributes & REPARSE_POINT != 0 && fs::is_link_tag(tag) {
        return Err(refuse("is a link"));
    }
    if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(refuse("isn't a folder"));
    }
    if attributes & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0 {
        return Err(refuse("is a cloud placeholder"));
    }
    Ok(file)
}

/// Open a directory without following a link at `dir` itself.
///
/// What isn't a folder, can't be listed, or is a cloud placeholder is an error.
pub fn open_dir(dir: &Path) -> io::Result<OwnedHandle> {
    Ok(open_listing(dir)?.into())
}

/// How much to ask the file system for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// Everything there is.
    Full,
    /// Only what's cheap. The same here: nothing costs more.
    Sizes,
}

/// List `dir` with full attributes. Links are reported, never followed.
pub fn read_dir(dir: &Path) -> io::Result<Vec<Entry>> {
    read_dir_with(dir, Detail::Full)
}

pub fn read_dir_with(dir: &Path, _detail: Detail) -> io::Result<Vec<Entry>> {
    let file = open_listing(dir)?;
    let dev = fs::by_handle(&file)?.dwVolumeSerialNumber as i32;
    let mut out = Vec::new();
    each_record(&file, |e, name| out.push(entry(dir, dev, name, e)))?;
    Ok(out)
}

/// The names in `dir`, from the same checked open as [`read_dir`] and nothing
/// else read of what's in it: no entry is sized and no file is opened.
pub fn names(dir: &Path) -> io::Result<Vec<OsString>> {
    let file = open_listing(dir)?;
    let mut out = Vec::new();
    each_record(&file, |_, name| out.push(name))?;
    Ok(out)
}

/// Calls `visit` with every record of the open folder `file` and its name
/// (`.` and `..` left out).
fn each_record(
    file: &File,
    mut visit: impl FnMut(&FILE_ID_EXTD_DIR_INFO, OsString),
) -> io::Result<()> {
    let handle = file.as_raw_handle() as HANDLE;
    // 8-byte aligned, as the records are.
    let mut buf = vec![0u64; 32 * 1024];
    let mut class = FileIdExtdDirectoryRestartInfo;
    loop {
        // SAFETY: `buf` is as long as the length passed; the handle is the
        // open folder's, for as long as `file` is.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                class,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
            )
        };
        class = FileIdExtdDirectoryInfo;
        if ok == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                break;
            }
            return Err(err);
        }
        let base = buf.as_ptr().cast::<u8>();
        let len = buf.len() * 8;
        let header = std::mem::size_of::<FILE_ID_EXTD_DIR_INFO>();
        let name_offset = std::mem::offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
        let bad = |why: &str| io::Error::new(io::ErrorKind::InvalidData, why.to_string());
        let mut at = 0usize;
        loop {
            // The records are the file system's, but nothing read from them is
            // trusted: each header must lie inside the buffer, at the 8-byte
            // offset the records have, before a field of it is read.
            if at & 7 != 0 || at + header > len {
                return Err(bad("a record outside the listing"));
            }
            // SAFETY: the header lies inside `buf` (just checked), `at` is
            // 8-byte aligned in a buffer that is, and the type has no invalid
            // bit pattern; a copy, so no reference is made to the buffer.
            let e = unsafe {
                base.add(at)
                    .cast::<FILE_ID_EXTD_DIR_INFO>()
                    .read_unaligned()
            };
            let units = e.FileNameLength as usize / 2;
            let name_at = at + name_offset;
            if name_at + units * 2 > len {
                return Err(bad("a name past the listing"));
            }
            // SAFETY: from the buffer itself, and the range was just checked
            // to lie inside it; 2-byte aligned, as `name_at` is even.
            let name = OsString::from_wide(unsafe {
                std::slice::from_raw_parts(base.add(name_at).cast::<u16>(), units)
            });
            if name != "." && name != ".." {
                visit(&e, name);
            }
            let next = e.NextEntryOffset as usize;
            if next == 0 {
                break;
            }
            // Forward, past this header, and still inside the buffer.
            if next < header || at + next >= len {
                return Err(bad("a record offset outside the listing"));
            }
            at += next;
        }
    }
    Ok(())
}

fn entry(dir: &Path, dev: i32, name: OsString, e: &FILE_ID_EXTD_DIR_INFO) -> Entry {
    let attributes = e.FileAttributes;
    let kind = if attributes & REPARSE_POINT != 0 && fs::is_link_tag(e.ReparsePointTag) {
        Kind::Symlink
    } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        Kind::Dir
    } else {
        Kind::File
    };
    // Like the macOS listing, folders carry no sizes.
    let (logical, allocated) = if kind == Kind::Dir {
        (0, 0)
    } else {
        (e.EndOfFile.max(0) as u64, e.AllocationSize.max(0) as u64)
    };
    let mut id = [0u8; 8];
    id.copy_from_slice(&e.FileId.Identifier[..8]);
    let inode = u64::from_le_bytes(id);
    let mut nlink = 1;
    if kind == Kind::File
        && allocated >= LINK_MIN
        && attributes & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) == 0
    {
        nlink = fs::link_count_of(&dir.join(&name), dev as u32, inode)
            .map_or(LINKS_UNKNOWN, |n| n.max(1));
    }
    Entry {
        name,
        kind,
        dev,
        crtime: unix_seconds(e.CreationTime),
        mtime: unix_seconds(e.LastWriteTime),
        atime: unix_seconds(e.LastAccessTime),
        flags: attributes,
        inode,
        nlink,
        logical,
        allocated,
        private: allocated,
        clone_id: 0,
        ext_flags: 0,
        clone_refcnt: 0,
        error: 0,
    }
}

/// A Windows time (100 ns since 1601) as Unix seconds.
fn unix_seconds(t: i64) -> i64 {
    t.div_euclid(10_000_000) - 11_644_473_600
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_name(entries: &[Entry], n: &str) -> Entry {
        entries.iter().find(|e| e.name == n).unwrap().clone()
    }

    #[test]
    fn a_listing_tells_files_from_folders_and_never_follows_a_link() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file"), noise(5000)).unwrap();
        std::fs::create_dir(dir.path().join("folder")).unwrap();
        let linked =
            std::os::windows::fs::symlink_file(dir.path().join("file"), dir.path().join("link"))
                .is_ok();
        let entries = read_dir(dir.path()).unwrap();
        let file = by_name(&entries, "file");
        assert_eq!(file.kind, Kind::File);
        // Its length, and what it takes, as an independent query of the file
        // says (whole clusters, or less on a compressed folder).
        assert_eq!(file.logical, 5000);
        assert_eq!(
            file.allocated,
            fs::file_info(&dir.path().join("file")).unwrap().allocated
        );
        assert!(file.allocated > 0);
        assert_eq!(file.private, file.allocated);
        assert_ne!(file.inode, 0);
        let folder = by_name(&entries, "folder");
        assert_eq!((folder.kind, folder.allocated), (Kind::Dir, 0));
        if linked {
            assert_eq!(by_name(&entries, "link").kind, Kind::Symlink);
        }
        let dev = fs::dev_of(dir.path()).unwrap();
        assert!(entries.iter().all(|e| e.dev == dev && e.error == 0));
        assert!(!file.is_dataless() && !file.may_share_blocks());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!((now - file.mtime).abs() < 120, "{} {now}", file.mtime);
        assert_eq!(entries.len(), 2 + usize::from(linked));
    }

    #[test]
    fn a_big_file_with_two_names_is_told_by_its_number_and_its_count() {
        let dir = tempfile::tempdir().unwrap();
        // Incompressible, so a compressed temp folder can't take it under LINK_MIN.
        let big = noise(2 << 20);
        std::fs::write(dir.path().join("a"), &big).unwrap();
        std::fs::hard_link(dir.path().join("a"), dir.path().join("b")).unwrap();
        std::fs::write(dir.path().join("small"), b"x").unwrap();
        std::fs::hard_link(dir.path().join("small"), dir.path().join("small2")).unwrap();
        let entries = read_dir(dir.path()).unwrap();
        let (a, b) = (by_name(&entries, "a"), by_name(&entries, "b"));
        assert!(a.allocated >= LINK_MIN, "{}", a.allocated);
        assert_eq!((a.nlink, b.nlink), (2, 2));
        assert_eq!(a.inode, b.inode);
        // A small file is taken to have one name.
        assert_eq!(by_name(&entries, "small").nlink, 1);
        assert_eq!(
            by_name(&entries, "small").inode,
            by_name(&entries, "small2").inode
        );
    }

    #[test]
    fn names_alone_are_listed_through_the_same_checks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("Sub")).unwrap();
        let mut names: Vec<_> = names(dir.path())
            .unwrap()
            .into_iter()
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["Sub", "a.txt"]);
        // What read_dir refuses, names refuses too: a file isn't a folder.
        assert!(names_of_file(&dir.path().join("a.txt")));
        assert!(super::names(&dir.path().join("missing")).is_err());
    }

    fn names_of_file(p: &Path) -> bool {
        super::names(p).is_err()
    }

    #[test]
    fn a_long_listing_is_read_in_pieces() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..3000 {
            std::fs::write(
                dir.path()
                    .join(format!("a-rather-long-file-name-{i:05}.txt")),
                b"x",
            )
            .unwrap();
        }
        let entries = read_dir(dir.path()).unwrap();
        assert_eq!(entries.len(), 3000);
        let mut names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 3000);
    }

    #[test]
    fn a_missing_folder_a_file_and_a_link_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_dir(&dir.path().join("missing")).is_err());
        let f = dir.path().join("f");
        std::fs::write(&f, "x").unwrap();
        let e = read_dir(&f).unwrap_err();
        assert!(e.to_string().contains("isn't a folder"), "{e}");
        assert!(open_dir(&f).is_err());
        std::fs::create_dir(dir.path().join("real")).unwrap();
        if std::os::windows::fs::symlink_dir(dir.path().join("real"), dir.path().join("link"))
            .is_ok()
        {
            let e = read_dir(&dir.path().join("link")).unwrap_err();
            assert!(e.to_string().contains("is a link"), "{e}");
        }
        assert!(open_dir(dir.path()).is_ok());
    }

    #[test]
    fn cloud_placeholders_are_told_by_their_attributes() {
        let e = |flags| Entry {
            flags,
            ..entry_of_nothing()
        };
        assert!(e(RECALL_ON_DATA_ACCESS).is_dataless());
        assert!(e(RECALL_ON_OPEN).is_dataless());
        assert!(e(OFFLINE).is_dataless());
        assert!(!e(0x20).is_dataless());
        assert!(e(SPARSE_FILE).is_sparse());
    }

    #[test]
    fn only_a_name_surrogate_tag_is_a_link() {
        // Symbolic link, junction, mount point and an LX symlink; not a cloud
        // file (0x9000601A) nor a compression tag.
        for tag in [0xA000_000Cu32, 0xA000_0003, 0xA000_001D] {
            assert!(fs::is_link_tag(tag), "{tag:#x}");
        }
        for tag in [0u32, 0x9000_601A, 0x8000_0017] {
            assert!(!fs::is_link_tag(tag), "{tag:#x}");
        }
    }

    #[test]
    fn windows_time_is_unix_time() {
        // 1970-01-01 and 2000-01-01 in 100 ns since 1601.
        assert_eq!(unix_seconds(116_444_736_000_000_000), 0);
        assert_eq!(unix_seconds(125_911_584_000_000_000), 946_684_800);
    }

    /// Bytes that don't compress.
    fn noise(n: usize) -> Vec<u8> {
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 32) as u8
            })
            .collect()
    }

    fn entry_of_nothing() -> Entry {
        Entry {
            name: OsString::new(),
            kind: Kind::File,
            dev: 0,
            crtime: 0,
            mtime: 0,
            atime: 0,
            flags: 0,
            inode: 0,
            nlink: 1,
            logical: 0,
            allocated: 0,
            private: 0,
            clone_id: 0,
            ext_flags: 0,
            clone_refcnt: 0,
            error: 0,
        }
    }
}
