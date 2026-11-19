//! What changed on a volume since a saved position: the neutral face of the
//! file-system change journal.
//!
//! NTFS keeps a journal (USN) of every change to a name, a size or a file. A
//! replay reads it from a saved position, as `FSCTL_READ_UNPRIVILEGED_USN_JOURNAL`
//! allows a standard user on a handle to the volume's root folder (measured on
//! a runner, Phase 0). A record names the folder that holds the file by its
//! number; the folder's path is looked up by that number, once for each.
//!
//! The saved position is one `u64`: a tag of the journal's id above the USN.
//! A journal that was deleted and made again has another id, so a position
//! saved under the old one is told apart, and is a gap, not a guess. A volume
//! without a journal, or without one this user may read, is a gap too.

use std::collections::HashMap;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_NO_RECALL, FILE_ID_DESCRIPTOR, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFinalPathNameByHandleW, OpenFileById,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_UNPRIVILEGED_USN_JOURNAL, READ_USN_JOURNAL_DATA_V0,
    USN_JOURNAL_DATA_V0, USN_RECORD_V2,
};

use crate::fs;

/// Event flags, the values FSEvents' are, so the code above doesn't care
/// which OS wrote them.
pub mod flag {
    /// Changes were coalesced or dropped below this path: rescan it whole.
    pub const MUST_SCAN_SUBDIRS: u32 = 0x1;
    pub const USER_DROPPED: u32 = 0x2;
    pub const KERNEL_DROPPED: u32 = 0x4;
    pub const IDS_WRAPPED: u32 = 0x8;
    /// Marks the end of the replay of past events.
    pub const HISTORY_DONE: u32 = 0x10;
    pub const ROOT_CHANGED: u32 = 0x20;
    pub const MOUNT: u32 = 0x40;
    pub const UNMOUNT: u32 = 0x80;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// The directory that changed.
    pub path: PathBuf,
    pub flags: u32,
    pub id: u64,
}

impl Event {
    /// Whether everything below `path` must be rescanned rather than just
    /// `path` itself.
    pub fn must_rescan(&self) -> bool {
        self.flags & (flag::MUST_SCAN_SUBDIRS | flag::USER_DROPPED | flag::KERNEL_DROPPED) != 0
    }
}

/// What a replay found.
#[derive(Debug, Default)]
pub struct Replay {
    /// The directories that changed since the position, oldest first.
    pub events: Vec<Event>,
    /// Whether the journal said it had delivered all of its history. When
    /// false there may be more: a gap.
    pub complete: bool,
    /// Where to resume next time, when it isn't the newest event's id: a
    /// journal that had nothing new still moved on. `None` where the events'
    /// ids say it (macOS).
    pub next: Option<u64>,
}

/// The position of "now": where a first replay starts from. It can't be told
/// without a volume, so 0, which the first replay turns into the journal's end.
pub fn now() -> u64 {
    0
}

/// Bits of a saved position that are the USN; the rest tag the journal's id.
const USN_BITS: u32 = 44;
const USN_MASK: u64 = (1 << USN_BITS) - 1;

/// A short tag of a journal's id, so a position from another one is told apart
/// (one in a million that a journal made again has the same).
fn tag(journal: u64) -> u64 {
    (journal ^ journal >> 20 ^ journal >> 40) & ((1 << (64 - USN_BITS)) - 1)
}

fn cursor(journal: u64, usn: i64) -> u64 {
    // Maps beyond 17 TB of journal wrap, and read as a journal that moved on.
    tag(journal) << USN_BITS | (usn.max(0) as u64 & USN_MASK)
}

/// A changed file: the folder that holds it (by number), and what was done.
#[derive(Debug, PartialEq, Eq)]
struct Record {
    parent: u64,
    usn: i64,
}

/// The USN to read from next, and the records in a buffer a read filled. Every
/// field is read only after its record is known to lie inside the buffer.
fn parse(buf: &[u8]) -> io::Result<(i64, Vec<Record>)> {
    let bad = |why: &str| io::Error::new(io::ErrorKind::InvalidData, why.to_string());
    let header = std::mem::offset_of!(USN_RECORD_V2, FileName);
    let next = buf
        .first_chunk::<8>()
        .map(|b| i64::from_le_bytes(*b))
        .ok_or_else(|| bad("a journal read without its next position"))?;
    let mut at = 8;
    let mut out = Vec::new();
    while at < buf.len() {
        if at + header > buf.len() {
            return Err(bad("a journal record outside the read"));
        }
        // SAFETY: the header lies inside `buf` (just checked); the type has no
        // invalid bit pattern, and a copy is made, so alignment doesn't matter.
        let r = unsafe {
            buf.as_ptr()
                .add(at)
                .cast::<USN_RECORD_V2>()
                .read_unaligned()
        };
        let len = r.RecordLength as usize;
        if len < header || at + len > buf.len() {
            return Err(bad("a journal record of a length that doesn't fit"));
        }
        // Version 2 is what the request this makes is answered with. Another
        // layout is skipped by its own length, not read.
        if r.MajorVersion == 2 {
            out.push(Record {
                parent: r.ParentFileReferenceNumber,
                usn: r.Usn,
            });
        }
        // Records are 8-byte aligned, and never of no length.
        at += len.max(8).next_multiple_of(8);
    }
    Ok((next, out))
}

/// A handle on the volume's root folder, which is what the journal is asked
/// through, closed on drop.
struct Volume {
    file: std::fs::File,
}

impl Volume {
    fn open(root: &Path) -> io::Result<Volume> {
        // No access at all is enough to ask the journal (measured, Phase 0).
        Ok(Volume {
            file: fs::open_with(root.ancestors().last().unwrap_or(root), 0)?,
        })
    }

    fn handle(&self) -> HANDLE {
        self.file.as_raw_handle() as HANDLE
    }

    /// The journal's id and its first and next USN; `None` if there is none for
    /// this user.
    fn query(&self) -> io::Result<Option<USN_JOURNAL_DATA_V0>> {
        // SAFETY: zeroed is valid for this plain struct; the sizes are the ones
        // passed; the handle is open.
        let mut data: USN_JOURNAL_DATA_V0 = unsafe { std::mem::zeroed() };
        let mut got = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                self.handle(),
                FSCTL_QUERY_USN_JOURNAL,
                null(),
                0,
                (&raw mut data).cast(),
                size_of::<USN_JOURNAL_DATA_V0>() as u32,
                &mut got,
                null_mut(),
            )
        };
        if ok != 0 {
            return Ok(Some(data));
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            // No journal here (1179), being deleted (1178), or not for this user
            // (5), or this isn't NTFS (1: not supported).
            Some(1178 | 1179 | 5 | 1) => Ok(None),
            _ => Err(err),
        }
    }

    /// The records from `start`, and where the next read goes. `None` if the
    /// journal no longer holds `start`.
    fn read(
        &self,
        journal: u64,
        start: i64,
        buf: &mut [u8],
    ) -> io::Result<Option<(i64, Vec<Record>)>> {
        let req = READ_USN_JOURNAL_DATA_V0 {
            StartUsn: start,
            ReasonMask: u32::MAX,
            ReturnOnlyOnClose: 0,
            Timeout: 0,
            BytesToWaitFor: 0,
            UsnJournalID: journal,
        };
        let mut got = 0u32;
        // SAFETY: the buffers are the sizes passed; the handle is open.
        let ok = unsafe {
            DeviceIoControl(
                self.handle(),
                FSCTL_READ_UNPRIVILEGED_USN_JOURNAL,
                (&raw const req).cast(),
                size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut got,
                null_mut(),
            )
        };
        if ok == 0 {
            let err = io::Error::last_os_error();
            return match err.raw_os_error() {
                // The entry was deleted (1181: the journal wrapped), or the id
                // is another journal's (87).
                Some(1181 | 87) => Ok(None),
                _ => Err(err),
            };
        }
        parse(&buf[..got as usize]).map(Some)
    }

    /// The folder numbered `id` on this volume, as a plain path; `None` once it
    /// isn't there any more (or isn't ours to open).
    fn folder(&self, id: u64) -> Option<PathBuf> {
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
                self.handle(),
                &desc,
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null(),
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_NO_RECALL,
            )
        };
        if h.is_null() || h as isize == -1 {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let path = loop {
            // SAFETY: the handle is open; `buf` is as long as passed.
            let n = unsafe { GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, 0) }
                as usize;
            if n == 0 {
                break None;
            }
            if n < buf.len() {
                break Some(std::ffi::OsString::from_wide(&buf[..n]));
            }
            buf.resize(n + 1, 0);
        };
        // SAFETY: the handle is ours, and closed once.
        unsafe { CloseHandle(h) };
        path.map(|p| plain(PathBuf::from(p)))
    }
}

/// `\\?\C:\x` as `C:\x`, and `\\?\UNC\s\x` as `\\s\x`: the paths the rest of the
/// program uses.
fn plain(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path
    }
}

/// Whether `path` is `root` or below it, without regard to case.
fn under(root: &Path, path: &Path) -> bool {
    let lower = |p: &Path| {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect::<Vec<_>>()
    };
    let (root, path) = (lower(root), lower(path));
    path.starts_with(&root)
}

/// How many folders one replay looks up, which is what it costs: past it the
/// rest is a gap, and the next replay goes on from there.
const MAX_FOLDERS: usize = 50_000;

/// Replay the journal under `root` from position `since` (0: from now, which
/// takes the journal's end as the start and finds nothing). The folders that
/// changed come oldest first; a journal that can't be read, or doesn't reach
/// back to `since`, is a gap, never a guess.
pub fn replay(root: &Path, since: u64, timeout: Duration) -> io::Result<Replay> {
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the replay timeout is too long",
        )
    })?;
    let volume = Volume::open(root)?;
    let Some(journal) = volume.query()? else {
        return Ok(Replay::default());
    };
    let id = journal.UsnJournalID;
    let end = cursor(id, journal.NextUsn);
    let gap = |next| Replay {
        events: vec![Event {
            path: root.to_path_buf(),
            flags: flag::MUST_SCAN_SUBDIRS,
            id: next,
        }],
        complete: false,
        next: Some(next),
    };
    if since == 0 {
        return Ok(Replay {
            complete: true,
            next: Some(end),
            ..Replay::default()
        });
    }
    let start = (since & USN_MASK) as i64;
    // Another journal, or one that has been cut short past or reset before
    // that position: what happened in between is lost.
    if since >> USN_BITS != tag(id) || start < journal.FirstUsn || start > journal.NextUsn {
        return Ok(gap(end));
    }
    let mut out = Replay::default();
    let (mut at, mut buf) = (start, vec![0u8; 256 * 1024]);
    let mut paths: HashMap<u64, Option<PathBuf>> = HashMap::new();
    let mut complete = true;
    while at < journal.NextUsn {
        if Instant::now() >= deadline || paths.len() >= MAX_FOLDERS {
            complete = false;
            break;
        }
        let Some((next, records)) = volume.read(id, at, &mut buf)? else {
            return Ok(gap(end));
        };
        for r in &records {
            let path = paths
                .entry(r.parent)
                .or_insert_with(|| volume.folder(r.parent).filter(|p| under(root, p)))
                .clone();
            if let Some(path) = path {
                out.events.push(Event {
                    path,
                    flags: 0,
                    id: cursor(id, r.usn),
                });
            }
        }
        if next <= at {
            // Nothing moved: the end.
            break;
        }
        at = next;
    }
    out.complete = complete;
    out.next = Some(cursor(id, at));
    // One event for each folder is what the caller wants, newest position kept.
    let mut seen: HashMap<PathBuf, usize> = HashMap::new();
    let mut unique: Vec<Event> = Vec::new();
    for e in out.events.drain(..) {
        match seen.get(&e.path) {
            Some(&i) => unique[i].id = e.id,
            None => {
                seen.insert(e.path.clone(), unique.len());
                unique.push(e);
            }
        }
    }
    out.events = unique;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(parent: u64, usn: i64, name: &str) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let header = std::mem::offset_of!(USN_RECORD_V2, FileName);
        let len = (header + units.len() * 2).next_multiple_of(8);
        let mut r = vec![0u8; len];
        r[0..4].copy_from_slice(&(len as u32).to_le_bytes());
        r[4..6].copy_from_slice(&2u16.to_le_bytes());
        r[16..24].copy_from_slice(&parent.to_le_bytes());
        r[24..32].copy_from_slice(&usn.to_le_bytes());
        r[56..58].copy_from_slice(&((units.len() * 2) as u16).to_le_bytes());
        r[58..60].copy_from_slice(&(header as u16).to_le_bytes());
        for (i, u) in units.iter().enumerate() {
            r[header + i * 2..header + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        r
    }

    #[test]
    fn a_position_names_the_journal_it_came_from() {
        let c = cursor(0xDEAD_BEEF_1234, 987_654_321);
        assert_eq!(c & USN_MASK, 987_654_321);
        assert_eq!(c >> USN_BITS, tag(0xDEAD_BEEF_1234));
        assert_ne!(tag(0xDEAD_BEEF_1234), tag(0xDEAD_BEEF_1235));
        // A journal id that is only different in its high bits is told apart too.
        assert_ne!(tag(1), tag(1 << 41));
        assert_eq!(cursor(5, -3) & USN_MASK, 0);
    }

    #[test]
    fn records_are_read_by_their_lengths() {
        let mut buf = 4096i64.to_le_bytes().to_vec();
        buf.extend(record(100, 10, "a.txt"));
        buf.extend(record(200, 20, "a much longer name than before.log"));
        buf.extend(record(300, 30, "z"));
        let (next, rs) = parse(&buf).unwrap();
        assert_eq!(next, 4096);
        assert_eq!(
            rs,
            [
                Record {
                    parent: 100,
                    usn: 10
                },
                Record {
                    parent: 200,
                    usn: 20
                },
                Record {
                    parent: 300,
                    usn: 30
                },
            ]
        );
        // Nothing but the position: no records.
        assert_eq!(parse(&4096i64.to_le_bytes()).unwrap(), (4096, Vec::new()));
    }

    #[test]
    fn a_record_that_doesnt_fit_is_an_error_not_a_read_past_the_buffer() {
        let mut buf = 0i64.to_le_bytes().to_vec();
        buf.extend(record(1, 1, "a"));
        // Cut short in the middle of the record.
        assert!(parse(&buf[..buf.len() - 4]).is_err());
        // Cut inside the header.
        assert!(parse(&buf[..20]).is_err());
        // A length longer than what was read, and one shorter than a header.
        let mut long = buf.clone();
        long[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&long).is_err());
        let mut short = buf.clone();
        short[8..12].copy_from_slice(&8u32.to_le_bytes());
        assert!(parse(&short).is_err());
        // No position at all.
        assert!(parse(&[1, 2, 3]).is_err());
    }

    #[test]
    fn another_layout_of_record_is_skipped_by_its_length() {
        let mut buf = 77i64.to_le_bytes().to_vec();
        let mut v3 = record(1, 1, "x");
        v3[4..6].copy_from_slice(&3u16.to_le_bytes());
        buf.extend(v3);
        buf.extend(record(9, 9, "y"));
        let (_, rs) = parse(&buf).unwrap();
        assert_eq!(rs, [Record { parent: 9, usn: 9 }]);
    }

    #[test]
    fn paths_are_plain_and_compared_without_case() {
        assert_eq!(
            plain(PathBuf::from(r"\\?\C:\Users\x")),
            PathBuf::from(r"C:\Users\x")
        );
        assert_eq!(
            plain(PathBuf::from(r"\\?\UNC\srv\share\d")),
            PathBuf::from(r"\\srv\share\d")
        );
        assert_eq!(plain(PathBuf::from(r"C:\a")), PathBuf::from(r"C:\a"));
        assert!(under(Path::new(r"C:\"), Path::new(r"c:\Users\X")));
        assert!(under(Path::new(r"C:\Users"), Path::new(r"c:\users")));
        assert!(!under(Path::new(r"C:\Users"), Path::new(r"C:\UsersX")));
        assert!(!under(Path::new(r"C:\Users"), Path::new(r"D:\Users")));
    }

    #[test]
    fn dropped_events_mean_a_rescan() {
        let e = |flags| Event {
            path: PathBuf::from(r"C:\x"),
            flags,
            id: 1,
        };
        assert!(e(flag::MUST_SCAN_SUBDIRS).must_rescan());
        assert!(e(flag::KERNEL_DROPPED).must_rescan());
        assert!(!e(0).must_rescan());
    }

    /// The NTFS journal of the volume the temp folder is on: where the changes
    /// to a known folder are found again from a saved position. Skipped (and
    /// said) where the volume has no journal this user may read.
    #[test]
    fn a_change_is_found_again_from_a_saved_position() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let volume_root = fs::statfs(&root).unwrap().mount_point;
        let baseline = replay(Path::new(&volume_root), 0, Duration::from_secs(10)).unwrap();
        let Some(saved) = baseline.next else {
            eprintln!("no readable journal on {volume_root}: skipped");
            return;
        };
        assert!(baseline.complete && baseline.events.is_empty());
        // Nothing has changed under a folder that hasn't been touched.
        let busy = root.join("busy");
        let quiet = root.join("quiet");
        std::fs::create_dir(&quiet).unwrap();
        std::fs::create_dir(&busy).unwrap();
        let first = replay(Path::new(&volume_root), saved, Duration::from_secs(10)).unwrap();
        assert!(first.complete, "{first:?}");
        let in_dir = |r: &Replay, d: &Path| {
            r.events.iter().any(|e| {
                e.path
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&d.to_string_lossy())
            })
        };
        // The new folders' parent is where they were made.
        assert!(in_dir(&first, &root), "{first:?}");
        // Resume from where that left off: only the next change is found.
        let saved = first.next.unwrap();
        std::fs::write(busy.join("log.txt"), vec![1u8; 100_000]).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(busy.join("log.txt"))
            .unwrap();
        let second = replay(Path::new(&volume_root), saved, Duration::from_secs(10)).unwrap();
        assert!(second.complete, "{second:?}");
        assert!(in_dir(&second, &busy), "{second:?}");
        assert!(!in_dir(&second, &quiet), "{second:?}");
        // Each folder once, and the position moved forward.
        let mut paths: Vec<_> = second
            .events
            .iter()
            .map(|e| e.path.to_string_lossy().to_lowercase())
            .collect();
        paths.sort();
        let n = paths.len();
        paths.dedup();
        assert_eq!(n, paths.len());
        assert!(second.next.unwrap() >= saved);
        // Under `busy` alone, the rest of the volume's changes are left out.
        let only = replay(&busy, saved, Duration::from_secs(10)).unwrap();
        assert!(
            only.events.iter().all(|e| under(&busy, &e.path)),
            "{only:?}"
        );
        // A position of another journal is a gap, not a guess.
        let other = replay(
            Path::new(&volume_root),
            saved ^ (1 << 60),
            Duration::from_secs(10),
        )
        .unwrap();
        assert!(
            !other.complete && other.events[0].must_rescan(),
            "{other:?}"
        );
    }

    #[test]
    fn what_changes_a_closed_timeout_is_an_error() {
        let e = replay(Path::new(r"C:\"), now(), Duration::MAX).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    }
}
