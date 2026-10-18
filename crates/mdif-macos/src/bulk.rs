//! Directory listing via `getattrlistbulk(2)`.
//!
//! One call returns a batch of entries with everything the scanner needs:
//! logical and allocated size, APFS *private* size (bytes freed if the file
//! were deleted, i.e. not shared with a clone or trapped in a snapshot), the
//! clone family, and the dataless flag of cloud placeholders.
//!
//! `FSOPT_PACK_INVAL_ATTRS` makes every requested attribute present (zero
//! when invalid) in the order getattrlist(2) documents: common, file, then
//! common-extended attributes, each 4-byte aligned, then the name. One
//! exception, observed on macOS 27: directories carry no file-attribute
//! group at all (their records are 16 bytes shorter), so the layout depends on
//! the object type.

use std::ffi::{CString, OsString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

use libc::{c_int, c_void};

// <sys/attr.h>
const ATTR_BIT_MAP_COUNT: u16 = 5;
const ATTR_CMN_NAME: u32 = 0x0000_0001;
const ATTR_CMN_DEVID: u32 = 0x0000_0002;
const ATTR_CMN_OBJTYPE: u32 = 0x0000_0008;
const ATTR_CMN_CRTIME: u32 = 0x0000_0200;
const ATTR_CMN_MODTIME: u32 = 0x0000_0400;
const ATTR_CMN_FLAGS: u32 = 0x0004_0000;
const ATTR_CMN_FILEID: u32 = 0x0200_0000;
const ATTR_CMN_ERROR: u32 = 0x2000_0000;
const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;
const ATTR_FILE_LINKCOUNT: u32 = 0x0000_0001;
const ATTR_FILE_TOTALSIZE: u32 = 0x0000_0002;
const ATTR_FILE_ALLOCSIZE: u32 = 0x0000_0004;
const ATTR_CMNEXT_PRIVATESIZE: u32 = 0x0000_0008;
const ATTR_CMNEXT_CLONEID: u32 = 0x0000_0100;
const ATTR_CMNEXT_EXT_FLAGS: u32 = 0x0000_0200;
const ATTR_CMNEXT_CLONE_REFCNT: u32 = 0x0000_1000;
const FSOPT_NOFOLLOW: u64 = 0x01;
const FSOPT_PACK_INVAL_ATTRS: u64 = 0x08;
const FSOPT_ATTR_CMN_EXTENDED: u64 = 0x20;

/// `st_flags` bit of a cloud placeholder whose content is not on disk.
pub const SF_DATALESS: u32 = 0x4000_0000;
/// `ATTR_CMNEXT_EXT_FLAGS` bits.
pub const EF_MAY_SHARE_BLOCKS: u64 = 0x01;
pub const EF_IS_PURGEABLE: u64 = 0x08;
pub const EF_IS_SPARSE: u64 = 0x10;

// <sys/vnode.h> enum vtype
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;

#[repr(C)]
struct AttrList {
    bitmapcount: u16,
    reserved: u16,
    commonattr: u32,
    volattr: u32,
    dirattr: u32,
    fileattr: u32,
    forkattr: u32,
}

unsafe extern "C" {
    fn getattrlistbulk(
        dirfd: c_int,
        alist: *mut c_void,
        attribute_buffer: *mut c_void,
        buffer_size: usize,
        options: u64,
    ) -> c_int;
}

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
    /// Device the entry lives on; differs from its parent at a mount point.
    pub dev: i32,
    /// Creation and modification time, seconds since the epoch.
    pub crtime: i64,
    pub mtime: i64,
    /// BSD `st_flags`.
    pub flags: u32,
    /// Inode number: the same for every hard link to a file.
    pub inode: u64,
    /// Hard links to this file on the volume (1 for directories).
    pub nlink: u32,
    pub logical: u64,
    pub allocated: u64,
    /// Bytes freed immediately if this file were deleted.
    pub private: u64,
    /// Same value for files that are pure clones of each other.
    pub clone_id: u64,
    pub ext_flags: u64,
    /// Number of full clones sharing all of this file's blocks.
    pub clone_refcnt: u32,
    /// Per-entry error from the kernel (0 = none).
    pub error: u32,
}

impl Entry {
    /// Cloud placeholder: listed, but its content is not on this disk.
    pub fn is_dataless(&self) -> bool {
        self.flags & SF_DATALESS != 0
    }

    /// Allocated bytes shared with a clone or trapped in a snapshot.
    pub fn shared(&self) -> u64 {
        self.allocated.saturating_sub(self.private)
    }

    pub fn is_purgeable(&self) -> bool {
        self.ext_flags & EF_IS_PURGEABLE != 0
    }

    pub fn is_sparse(&self) -> bool {
        self.ext_flags & EF_IS_SPARSE != 0
    }
}

/// Open a directory without following a symlink at `dir` itself.
pub fn open_dir(dir: &Path) -> io::Result<OwnedFd> {
    let c = CString::new(dir.as_os_str().as_bytes())?;
    let fd = unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// How much to ask the file system for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// Everything, including private size and clone ID.
    Full,
    /// No private size or clone ID (left as `allocated` and 0). APFS
    /// computes private size per file, which makes up most of the cost of a
    /// listing: for re-listing busy folders often.
    Sizes,
}

/// List `dir` with full attributes. Symlinks are reported, never followed.
pub fn read_dir(dir: &Path) -> io::Result<Vec<Entry>> {
    read_dir_fd(&open_dir(dir)?)
}

pub fn read_dir_with(dir: &Path, detail: Detail) -> io::Result<Vec<Entry>> {
    read_dir_fd_with(&open_dir(dir)?, detail)
}

pub fn read_dir_fd(fd: &OwnedFd) -> io::Result<Vec<Entry>> {
    read_dir_fd_with(fd, Detail::Full)
}

pub fn read_dir_fd_with(fd: &OwnedFd, detail: Detail) -> io::Result<Vec<Entry>> {
    let full = detail == Detail::Full;
    let mut alist = AttrList {
        bitmapcount: ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: ATTR_CMN_RETURNED_ATTRS
            | ATTR_CMN_ERROR
            | ATTR_CMN_NAME
            | ATTR_CMN_DEVID
            | ATTR_CMN_OBJTYPE
            | ATTR_CMN_CRTIME
            | ATTR_CMN_MODTIME
            | ATTR_CMN_FLAGS
            | ATTR_CMN_FILEID,
        volattr: 0,
        dirattr: 0,
        fileattr: ATTR_FILE_LINKCOUNT | ATTR_FILE_TOTALSIZE | ATTR_FILE_ALLOCSIZE,
        forkattr: if full {
            ATTR_CMNEXT_PRIVATESIZE | ATTR_CMNEXT_CLONEID
        } else {
            0
        } | ATTR_CMNEXT_EXT_FLAGS
            | ATTR_CMNEXT_CLONE_REFCNT,
    };
    let options = FSOPT_NOFOLLOW | FSOPT_PACK_INVAL_ATTRS | FSOPT_ATTR_CMN_EXTENDED;
    let mut buf = vec![0u8; 256 * 1024];
    let mut out = Vec::new();
    loop {
        let n = unsafe {
            getattrlistbulk(
                fd.as_raw_fd(),
                (&raw mut alist).cast(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                options,
            )
        };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if n == 0 {
            return Ok(out);
        }
        let mut off = 0;
        for _ in 0..n {
            let len = Cursor::new(&buf[off..]).u32()? as usize;
            let rec = buf.get(off..off + len).ok_or_else(bad_record)?;
            out.push(parse(rec, full)?);
            off += len;
        }
    }
}

fn parse(rec: &[u8], full: bool) -> io::Result<Entry> {
    let mut c = Cursor::new(rec);
    c.skip(4); // record length
    // attribute_set_t of what is actually valid for this entry.
    let [_common, _vol, _dir, _file, ret_ext] = [c.u32()?, c.u32()?, c.u32()?, c.u32()?, c.u32()?];
    let error = c.u32()?;
    let name_at = c.pos;
    let name_off = c.i32()?;
    let name_len = c.u32()? as usize;
    let dev = c.i32()?;
    let objtype = c.u32()?;
    let crtime = c.i64()?;
    c.skip(8); // tv_nsec
    let mtime = c.i64()?;
    c.skip(8); // tv_nsec
    let flags = c.u32()?;
    let inode = c.u64()?;
    let (nlink, logical, allocated) = if objtype == VDIR {
        (1, 0, 0)
    } else {
        (c.u32()?, c.u64()?, c.u64()?)
    };
    let (private, clone_id) = if full {
        (c.u64()?, c.u64()?)
    } else {
        (allocated, 0)
    };
    let ext_flags = c.u64()?;
    let clone_refcnt = c.u32()?;

    // attrreference: offset is relative to the attrreference itself; the
    // length includes the trailing NUL.
    let start = usize::try_from(name_at as i64 + name_off as i64).map_err(|_| bad_record())?;
    let name = rec
        .get(start..start + name_len.saturating_sub(1))
        .ok_or_else(bad_record)?;

    let kind = match objtype {
        VREG => Kind::File,
        VDIR => Kind::Dir,
        VLNK => Kind::Symlink,
        _ => Kind::Other,
    };
    // Volumes without private-size support (non-APFS): nothing is shared.
    let private = if !full || ret_ext & ATTR_CMNEXT_PRIVATESIZE != 0 {
        private.min(allocated)
    } else {
        allocated
    };
    Ok(Entry {
        name: OsString::from_vec(name.to_vec()),
        kind,
        dev,
        crtime,
        mtime,
        flags,
        inode,
        nlink,
        logical,
        allocated,
        private,
        clone_id,
        ext_flags,
        clone_refcnt,
        error,
    })
}

fn bad_record() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "malformed getattrlistbulk record",
    )
}

/// Reads native-endian fields; attributes are only 4-byte aligned, so every
/// read copies bytes instead of casting pointers.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Cursor { buf, pos: 0 }
    }

    fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    fn take<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        let bytes = self
            .buf
            .get(self.pos..self.pos + N)
            .ok_or_else(bad_record)?;
        self.pos += N;
        Ok(bytes.try_into().expect("slice length is N"))
    }

    fn u32(&mut self) -> io::Result<u32> {
        self.take().map(u32::from_ne_bytes)
    }

    fn i32(&mut self) -> io::Result<i32> {
        self.take().map(i32::from_ne_bytes)
    }

    fn u64(&mut self) -> io::Result<u64> {
        self.take().map(u64::from_ne_bytes)
    }

    fn i64(&mut self) -> io::Result<i64> {
        self.take().map(i64::from_ne_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::process::Command;

    fn entry<'a>(entries: &'a [Entry], name: &str) -> &'a Entry {
        entries
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("no entry {name}"))
    }

    #[test]
    fn matches_lstat_and_sees_clones_and_sparse_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let data: Vec<u8> = (0..4_000_000u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
            .collect();
        fs::write(p.join("orig.bin"), &data).unwrap();
        fs::write(p.join("copy.bin"), &data).unwrap();
        // cp -c clones (clonefile); std has no clone API.
        assert!(
            Command::new("cp")
                .args(["-c", "orig.bin", "clone.bin"])
                .current_dir(p)
                .status()
                .unwrap()
                .success()
        );
        // A 1 GB file with no data written: sparse.
        fs::File::create(p.join("sparse.bin"))
            .unwrap()
            .set_len(1 << 30)
            .unwrap();
        fs::create_dir(p.join("sub")).unwrap();
        std::os::unix::fs::symlink("orig.bin", p.join("link")).unwrap();
        fs::hard_link(p.join("copy.bin"), p.join("hardlink.bin")).unwrap();

        let entries = read_dir(p).unwrap();
        assert_eq!(entries.len(), 7);
        let (copy, hard) = (entry(&entries, "copy.bin"), entry(&entries, "hardlink.bin"));
        assert_eq!((copy.inode, copy.nlink), (hard.inode, 2));

        for e in &entries {
            let md = fs::symlink_metadata(p.join(&e.name)).unwrap();
            assert_eq!(e.dev as i64, md.dev() as i64, "{:?}", e.name);
            assert_eq!(e.mtime, md.mtime(), "{:?}", e.name);
            assert_eq!(e.inode, md.ino(), "{:?}", e.name);
            if e.kind != Kind::Dir {
                assert_eq!(e.nlink as u64, md.nlink(), "{:?}", e.name);
            }
            let created = md.created().unwrap().duration_since(std::time::UNIX_EPOCH);
            assert_eq!(e.crtime, created.unwrap().as_secs() as i64, "{:?}", e.name);
            if e.kind == Kind::File {
                assert_eq!(e.logical, md.size(), "{:?}", e.name);
                assert_eq!(e.allocated, md.blocks() * 512, "{:?}", e.name);
            }
        }

        let (orig, clone, copy) = (
            entry(&entries, "orig.bin"),
            entry(&entries, "clone.bin"),
            entry(&entries, "copy.bin"),
        );
        assert_eq!(orig.clone_id, clone.clone_id);
        assert_ne!(orig.clone_id, copy.clone_id);
        assert_eq!(orig.private, 0, "clone shares every block");
        assert_eq!(clone.private, 0);
        assert_eq!(
            copy.private, copy.allocated,
            "independent copy owns its blocks"
        );
        assert_eq!(orig.shared(), orig.allocated);

        let sparse = entry(&entries, "sparse.bin");
        assert_eq!(sparse.logical, 1 << 30);
        assert!(
            sparse.allocated < 1 << 20,
            "sparse file allocates almost nothing"
        );

        assert_eq!(entry(&entries, "sub").kind, Kind::Dir);
        assert_eq!(entry(&entries, "link").kind, Kind::Symlink);
        assert!(entries.iter().all(|e| !e.is_dataless()));
    }

    #[test]
    fn many_entries_span_several_batches() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5000 {
            fs::write(
                dir.path()
                    .join(format!("file-with-a-longish-name-{i:05}.txt")),
                b"x",
            )
            .unwrap();
        }
        assert_eq!(read_dir(dir.path()).unwrap().len(), 5000);
    }

    #[test]
    fn missing_dir_is_an_error() {
        assert!(read_dir(Path::new("/nonexistent/mdif")).is_err());
    }
}
