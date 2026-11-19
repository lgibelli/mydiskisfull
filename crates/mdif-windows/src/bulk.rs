//! Listing a directory: each entry's kind, sizes, times and attributes.
//!
//! Every entry comes from the directory listing itself (the find data), so
//! no file is opened and no access time changes. Links are reported, never
//! followed (a junction counts as one), and a cloud placeholder is flagged
//! so callers neither enter nor count it.
//!
//! For now an entry's allocated size is its length, its file number is 0
//! and it has one name: hard links aren't told apart yet, and NTFS has no
//! clones, so private size is allocated size and nothing shares blocks. A
//! sparse or compressed file is counted at its length, and the listing is of
//! the path, checked a moment before: all of that waits for the real listing
//! from an open handle (`FileIdExtdDirectoryInfo`), which gives allocation
//! size, file number and link count too. Until then nothing that scans or
//! sizes folders runs on Windows.

use std::ffi::OsString;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::OwnedHandle;
use std::path::Path;
use std::time::SystemTime;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
};

use crate::fs;

/// `FILE_ATTRIBUTE_*` of cloud files and sparse files.
const OFFLINE: u32 = 0x0000_1000;
const RECALL_ON_OPEN: u32 = 0x0004_0000;
const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
const SPARSE_FILE: u32 = 0x0000_0200;

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

/// Open a directory without following a link at `dir` itself.
///
/// What isn't a folder, can't be listed, or is a cloud placeholder (whose
/// listing would be downloaded) is an error.
pub fn open_dir(dir: &Path) -> io::Result<OwnedHandle> {
    let refuse = |why: &str| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{}: {why}", dir.display()),
        )
    };
    if std::fs::symlink_metadata(dir)?.file_type().is_symlink() {
        return Err(refuse("is a link"));
    }
    let file = fs::open_with(dir, FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY)?;
    let attributes = fs::by_handle(&file)?.dwFileAttributes;
    if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(refuse("isn't a folder"));
    }
    if attributes & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0 {
        return Err(refuse("is a cloud placeholder"));
    }
    Ok(file.into())
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
    // Fails for a link or what isn't a folder, as the macOS listing does.
    drop(open_dir(dir)?);
    let dev = fs::dev_of(dir)?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Ok(meta) = entry.metadata() else {
            out.push(failed(name, dev));
            continue;
        };
        let kind = {
            let t = meta.file_type();
            if t.is_symlink() {
                Kind::Symlink
            } else if t.is_dir() {
                Kind::Dir
            } else if t.is_file() {
                Kind::File
            } else {
                Kind::Other
            }
        };
        // Like the macOS listing, folders carry no sizes.
        let logical = if kind == Kind::Dir {
            0
        } else {
            meta.file_size()
        };
        out.push(Entry {
            name,
            kind,
            dev,
            crtime: seconds(meta.created()),
            mtime: seconds(meta.modified()),
            atime: seconds(meta.accessed()),
            flags: meta.file_attributes(),
            inode: 0,
            nlink: 1,
            logical,
            allocated: logical,
            private: logical,
            clone_id: 0,
            ext_flags: 0,
            clone_refcnt: 0,
            error: 0,
        });
    }
    Ok(out)
}

/// An entry whose attributes couldn't be read.
fn failed(name: OsString, dev: i32) -> Entry {
    Entry {
        name,
        kind: Kind::Other,
        dev,
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
        error: 1,
    }
}

fn seconds(t: io::Result<SystemTime>) -> i64 {
    match t.map(|t| t.duration_since(SystemTime::UNIX_EPOCH)) {
        Ok(Ok(after)) => after.as_secs() as i64,
        Ok(Err(before)) => -(before.duration().as_secs() as i64),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_tells_files_from_folders_and_never_follows_a_link() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file"), vec![1u8; 5000]).unwrap();
        std::fs::create_dir(dir.path().join("folder")).unwrap();
        let linked =
            std::os::windows::fs::symlink_file(dir.path().join("file"), dir.path().join("link"))
                .is_ok();
        let mut entries = read_dir(dir.path()).unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let by = |n: &str| entries.iter().find(|e| e.name == n).unwrap();
        assert_eq!(by("file").kind, Kind::File);
        assert_eq!((by("file").logical, by("file").allocated), (5000, 5000));
        assert_eq!(by("folder").kind, Kind::Dir);
        assert_eq!(by("folder").allocated, 0);
        if linked {
            assert_eq!(by("link").kind, Kind::Symlink);
        }
        let dev = fs::dev_of(dir.path()).unwrap();
        assert!(entries.iter().all(|e| e.dev == dev && e.error == 0));
        assert!(!by("file").is_dataless() && !by("file").may_share_blocks());
        assert!(by("file").mtime > 1_000_000_000);
    }

    #[test]
    fn a_missing_folder_is_an_error_and_so_is_a_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_dir(&dir.path().join("missing")).is_err());
        let f = dir.path().join("f");
        std::fs::write(&f, "x").unwrap();
        assert!(read_dir(&f).is_err());
    }

    #[test]
    fn cloud_placeholders_are_told_by_their_attributes() {
        let e = |flags| Entry {
            flags,
            ..failed(OsString::new(), 0)
        };
        assert!(e(RECALL_ON_DATA_ACCESS).is_dataless());
        assert!(e(RECALL_ON_OPEN).is_dataless());
        assert!(e(OFFLINE).is_dataless());
        assert!(!e(0x20).is_dataless());
        assert!(e(SPARSE_FILE).is_sparse());
    }
}
