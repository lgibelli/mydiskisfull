//! Where a file's data physically lives (`F_LOG2PHYS_EXT`).
//!
//! Clone IDs only identify *pure* clones: once one copy is modified it gets
//! its own ID, yet still shares most of its blocks with the others. The
//! physical extents don't lie, so the union of the extents of files that
//! may share blocks is exactly the space they use together.
//!
//! Metadata only: the file is opened, never read, and cloud placeholders
//! are left to the caller to skip.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Physical extents of the regular file at `path`, as `(device offset,
/// length)` with lengths rounded up to `block`. Holes are left out.
pub fn physical(path: &Path, block: u64) -> io::Result<Vec<(u64, u64)>> {
    let c = CString::new(path.as_os_str().as_bytes())?;
    let fd = unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd.as_raw_fd(), st.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let st = unsafe { st.assume_init() };
    if st.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let block = block.max(1);
    let len = st.st_size;
    let mut out = Vec::new();
    let mut off: i64 = 0;
    while off < len {
        let mut l = libc::log2phys {
            l2p_flags: 0,
            l2p_contigbytes: len - off,
            l2p_devoffset: off,
        };
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_LOG2PHYS_EXT, &mut l) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let (n, dev) = (l.l2p_contigbytes, l.l2p_devoffset);
        if n <= 0 {
            return Err(io::Error::other("F_LOG2PHYS_EXT made no progress"));
        }
        // A hole has no device offset.
        if dev >= 0 {
            out.push((dev as u64, (n as u64).div_ceil(block) * block));
        }
        off += n;
    }
    Ok(out)
}

/// Bytes covered by `extents`, overlaps counted once. Sorts in place.
pub fn union_len(extents: &mut [(u64, u64)]) -> u64 {
    extents.sort_unstable();
    let mut total = 0;
    let mut cur: Option<(u64, u64)> = None;
    for &(start, len) in extents.iter() {
        let end = start + len;
        cur = match cur {
            Some((s, e)) if start <= e => Some((s, e.max(end))),
            Some((s, e)) => {
                total += e - s;
                Some((start, end))
            }
            None => Some((start, end)),
        };
    }
    total + cur.map_or(0, |(s, e)| e - s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};
    use std::process::Command;

    const MIB: u64 = 1 << 20;

    #[test]
    fn a_modified_clone_shares_the_untouched_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::write(&a, vec![7u8; 8 * MIB as usize]).unwrap();
        std::fs::File::open(&a).unwrap().sync_all().unwrap();
        assert!(
            Command::new("cp")
                .arg("-c")
                .arg(&a)
                .arg(&b)
                .status()
                .unwrap()
                .success()
        );
        let mut f = std::fs::OpenOptions::new().write(true).open(&b).unwrap();
        f.seek(SeekFrom::Start(2 * MIB)).unwrap();
        f.write_all(&vec![9u8; MIB as usize]).unwrap();
        f.sync_all().unwrap();

        let mut all = physical(&a, 4096).unwrap();
        assert_eq!(union_len(&mut all.clone()), 8 * MIB);
        all.extend(physical(&b, 4096).unwrap());
        assert_eq!(union_len(&mut all), 9 * MIB);
    }

    #[test]
    fn holes_take_no_space() {
        // APFS zero-fills up to the first write; the 60 MiB tail stays a
        // hole. (Small sparse files are different: APFS reserves their
        // whole length although the extents only cover what was written,
        // so callers must not rely on extents alone for those.)
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sparse");
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(64 * MIB).unwrap();
        std::os::unix::fs::FileExt::write_at(&f, &[1u8; 4096], 4 * MIB).unwrap();
        f.sync_all().unwrap();
        let allocated = std::os::unix::fs::MetadataExt::blocks(&f.metadata().unwrap()) * 512;
        let mut x = physical(&p, 4096).unwrap();
        assert!(allocated < 8 * MIB, "{allocated}");
        assert_eq!(union_len(&mut x), allocated, "{x:?}");
    }

    #[test]
    fn union_merges_overlaps() {
        let mut v = vec![(10, 10), (0, 5), (15, 10), (40, 1)];
        assert_eq!(union_len(&mut v), 5 + 15 + 1);
        assert_eq!(union_len(&mut []), 0);
    }
}
