//! What `std` only gives on Unix, behind one face: the facts of a file that
//! the scanners and clean-ups read (allocated bytes, which volume, file
//! identity, link count, times, owner), a few file-creation modes, and byte
//! strings as paths.
//!
//! The functions take where the entry is ([`At`]) as well as its metadata: on
//! Unix the metadata has it all, on Windows these facts come from a handle
//! opened on the file. A [`DirEntry`] gives its path only when asked, so a
//! walk that never needs one builds none.
//!
//! Nothing here reads a file's contents.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::fs::{DirEntry, File, Metadata, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt as _;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};

/// Where an entry is: a path, or a directory entry that knows its own.
pub trait At {
    fn path(&self) -> Cow<'_, Path>;
}

impl At for Path {
    fn path(&self) -> Cow<'_, Path> {
        Cow::Borrowed(self)
    }
}

impl At for PathBuf {
    fn path(&self) -> Cow<'_, Path> {
        Cow::Borrowed(self)
    }
}

impl At for DirEntry {
    fn path(&self) -> Cow<'_, Path> {
        Cow::Owned(DirEntry::path(self))
    }
}

/// Bytes the file takes on the disk (allocated, not its length).
pub fn allocated(_at: &(impl At + ?Sized), m: &Metadata) -> u64 {
    #[cfg(unix)]
    {
        m.blocks() * 512
    }
}

/// The volume the entry is on. Two entries of one volume give the same
/// value; 0 when it can't be told.
pub fn dev(_at: &(impl At + ?Sized), m: &Metadata) -> u64 {
    #[cfg(unix)]
    {
        m.dev()
    }
}

/// The file's identity on its volume: the same for every hard link to it.
/// 0 when it can't be told.
pub fn file_id(_at: &(impl At + ?Sized), m: &Metadata) -> u64 {
    #[cfg(unix)]
    {
        m.ino()
    }
}

/// How many names the file has (1 when it can't be told).
pub fn links(_at: &(impl At + ?Sized), m: &Metadata) -> u64 {
    #[cfg(unix)]
    {
        m.nlink()
    }
}

/// When the entry was last changed, in Unix seconds.
pub fn mtime(m: &Metadata) -> i64 {
    #[cfg(unix)]
    {
        m.mtime()
    }
}

/// When the entry was last read, in Unix seconds. Where the system doesn't
/// keep it up to date this is no later than it should be, never later.
pub fn atime(m: &Metadata) -> i64 {
    #[cfg(unix)]
    {
        m.atime()
    }
}

/// Whether the entry is a cloud placeholder, whose contents aren't on this
/// disk: listing or opening it would download them.
pub fn is_dataless(m: &Metadata) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt as _;
        m.st_flags() & crate::os::bulk::SF_DATALESS != 0
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = m;
        false
    }
}

/// The user who owns the entry, where the system has users by number.
pub fn uid(m: &Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        Some(m.uid())
    }
}

/// The user this process runs as, where the system has users by number.
pub fn current_uid() -> Option<u32> {
    #[cfg(unix)]
    {
        Some(unsafe { libc::getuid() })
    }
}

/// A name that sorts after every valid Unicode name, for jumping to the end
/// of a subtree in an ordered set of paths (paths compare by component).
/// Where names may be arbitrary bytes, one can still sort after it: callers
/// step over those.
pub fn name_after_all() -> &'static OsStr {
    #[cfg(unix)]
    {
        // No name holds 0xFF: UTF-8 never does.
        OsStr::from_bytes(b"\xff")
    }
}

/// A path as a program prints it in bytes (git's `-z` output). Not UTF-8 on
/// Unix; where paths are text it's the text, with what isn't replaced.
pub fn os_str_from_bytes(bytes: &[u8]) -> Cow<'_, OsStr> {
    #[cfg(unix)]
    {
        Cow::Borrowed(OsStr::from_bytes(bytes))
    }
}

/// The permission bits of the entry (`0o644`), as `with_mode` takes them.
pub fn mode_of(m: &Metadata) -> u32 {
    #[cfg(unix)]
    {
        m.permissions().mode() & 0o7777
    }
}

/// Create the file with these permission bits, where there are any.
pub fn with_mode(o: &mut OpenOptions, mode: u32) -> &mut OpenOptions {
    #[cfg(unix)]
    {
        o.mode(mode)
    }
}

/// Set the permission bits of an open file (the umask may have narrowed
/// them at creation), where there are any.
pub fn set_mode(f: &File, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        f.set_permissions(std::fs::Permissions::from_mode(mode))
    }
}

/// The user's home folder, from the environment (`$HOME`; the profile
/// folder on Windows). In the App Sandbox that is the app's container.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(crate::os::sys::account_home)
    }
}

/// "Not permitted", as the system says it, for what we refuse to open.
pub fn denied() -> io::Error {
    #[cfg(unix)]
    {
        io::Error::from_raw_os_error(libc::EPERM)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn facts_of_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::write(&a, vec![1u8; 10_000]).unwrap();
        std::fs::hard_link(&a, &b).unwrap();
        let (ma, mb) = (
            std::fs::symlink_metadata(&a).unwrap(),
            std::fs::symlink_metadata(&b).unwrap(),
        );
        assert_eq!(allocated(&a, &ma), ma.blocks() * 512);
        assert!(allocated(&a, &ma) > 0);
        assert_eq!(links(&a, &ma), 2);
        assert_eq!(file_id(&a, &ma), file_id(&b, &mb));
        assert_ne!(file_id(&a, &ma), 0);
        let md = std::fs::symlink_metadata(dir.path()).unwrap();
        assert_eq!(dev(&a, &ma), dev(dir.path(), &md));
        assert!(!is_dataless(&ma));
        assert_eq!(uid(&ma), current_uid());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!((now - mtime(&ma)).abs() < 60 && (now - atime(&ma)).abs() < 60);
    }

    #[test]
    fn modes_are_set_at_creation_whatever_the_umask() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("secret");
        let mut o = OpenOptions::new();
        let f = with_mode(o.write(true).create_new(true), 0o600)
            .open(&p)
            .unwrap();
        // Private from the moment it exists, not once set_mode has run.
        assert_eq!(mode_of(&std::fs::metadata(&p).unwrap()) & 0o077, 0);
        set_mode(&f, 0o600).unwrap();
        assert_eq!(mode_of(&std::fs::metadata(&p).unwrap()), 0o600);
    }

    #[test]
    fn the_last_name_sorts_after_any_name() {
        let end = Path::new("/a/b").join(name_after_all());
        for name in ["zzzz", "é", "~", "\u{10FFFF}"] {
            assert!(Path::new("/a/b").join(name) < end, "{name}");
        }
    }

    #[test]
    fn bytes_become_a_path_and_denied_is_permission_denied() {
        assert_eq!(os_str_from_bytes(b"x/y"), OsStr::new("x/y"));
        assert_eq!(denied().kind(), io::ErrorKind::PermissionDenied);
    }
}
