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
use std::mem::offset_of;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HANDLE;
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

/// The position of "now" on the volume holding `root`: where a first replay
/// starts from. 0 where the volume has no journal to read, which the first
/// replay turns into the journal's end.
pub fn now(root: &Path) -> u64 {
    Volume::open(root)
        .and_then(|v| v.query())
        .ok()
        .flatten()
        .map_or(0, |j| cursor(j.UsnJournalID, j.NextUsn))
}

/// The identity of the change journal of the volume holding `volume`: a
/// position only means something in the one it was taken in (the journal
/// deleted and made again starts over). `None` where there is none to read.
pub fn history(volume: &Path) -> Option<String> {
    Volume::open(&fs::normalized(volume))
        .and_then(|v| v.query())
        .ok()
        .flatten()
        .map(|j| format!("{:016x}", j.UsnJournalID))
}

/// Bits of a saved position that are the USN; the 19 above tag the journal's id
/// and keep the top bit clear (a position is kept as a signed number). A USN is
/// a record's offset in the journal, always a multiple of 8, so it's kept in
/// eights: 44 bits reach 140 TB of journal.
const USN_BITS: u32 = 44;
const USN_MASK: u64 = (1 << USN_BITS) - 1;

/// A short tag of a journal's id, so a position from another one is told apart
/// (one in half a million that a journal made again has the same).
fn tag(journal: u64) -> u64 {
    (journal ^ journal >> 19 ^ journal >> 38 ^ journal >> 57) & ((1 << (63 - USN_BITS)) - 1)
}

fn cursor(journal: u64, usn: i64) -> u64 {
    tag(journal) << USN_BITS | ((usn.max(0) as u64 >> 3) & USN_MASK)
}

/// The USN a position holds.
fn usn_of(position: u64) -> i64 {
    ((position & USN_MASK) << 3) as i64
}

/// A changed file: the folder that holds it (by number), and what was done.
#[derive(Debug, PartialEq, Eq)]
struct Record {
    parent: u64,
    usn: i64,
}

/// The USN to read from next, and the records in a buffer a read filled. A
/// record is read only after its whole length is known to lie inside the buffer,
/// and by byte ranges of it, so nothing is ever read past what the call wrote.
fn parse(buf: &[u8]) -> io::Result<(i64, Vec<Record>, usize)> {
    let bad = |why: &str| io::Error::new(io::ErrorKind::InvalidData, why.to_string());
    let header = std::mem::offset_of!(USN_RECORD_V2, FileName);
    let le = |b: &[u8], at: usize| -> Option<[u8; 8]> { b.get(at..at + 8)?.try_into().ok() };
    let next = le(buf, 0)
        .map(i64::from_le_bytes)
        .ok_or_else(|| bad("a journal read without its next position"))?;
    let mut at = 8;
    let (mut out, mut skipped) = (Vec::new(), 0);
    while at < buf.len() {
        let rest = &buf[at..];
        let len = rest
            .get(..4)
            .and_then(|b| b.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or_else(|| bad("a journal record outside the read"))? as usize;
        if len < header || len > rest.len() {
            return Err(bad("a journal record of a length that doesn't fit"));
        }
        let record = &rest[..len];
        let major = u16::from_le_bytes([record[4], record[5]]);
        // Version 2 is what the request this makes is answered with. Another
        // layout is skipped by its own length, not read.
        if major == 2 {
            let field = |at| i64::from_le_bytes(le(record, at).expect("inside the header"));
            out.push(Record {
                parent: field(offset_of!(USN_RECORD_V2, ParentFileReferenceNumber)) as u64,
                usn: field(offset_of!(USN_RECORD_V2, Usn)),
            });
        } else {
            skipped += 1;
        }
        // Records are 8-byte aligned.
        at += len.next_multiple_of(8);
    }
    Ok((next, out, skipped))
}

/// A handle on the volume's root folder, which is what the journal is asked
/// through, closed on drop.
struct Volume {
    file: std::fs::File,
}

impl Volume {
    fn open(root: &Path) -> io::Result<Volume> {
        // The volume that holds `root`, where it is mounted: a drive letter or a
        // folder. No access at all is enough to ask the journal (Phase 0).
        // By its own name when it has one: a volume mounted in a folder is
        // reached through that folder's reparse point, which would open the
        // containing volume's folder instead.
        let stats = fs::statfs(root)?;
        let at = if stats.device.starts_with(r"\\?\Volume{") {
            &stats.device
        } else {
            &stats.mount_point
        };
        Ok(Volume {
            file: fs::open_with(Path::new(at), 0)?,
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
    ) -> io::Result<Option<(i64, Vec<Record>, usize)>> {
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

    /// The folder numbered `id` on this volume, as a plain path (`Ok(None)` once
    /// it isn't there any more); see [`fs::folder_by_id`].
    fn folder(&self, id: u64) -> io::Result<Option<PathBuf>> {
        fs::folder_by_id(self.handle(), id)
    }
}

/// Whether `path` is `root` or below it. Both are spelled as the file system
/// spells them (the root is canonicalized, the folders come from it), so names
/// are compared exactly: folders that differ only in case are different ones
/// where a directory is case sensitive. Only the drive letter is folded.
fn under(root: &Path, path: &Path) -> bool {
    use std::path::Component::Prefix;
    let (mut root, mut path) = (root.components(), path.components());
    loop {
        match (root.next(), path.next()) {
            (None, _) => return true,
            (Some(Prefix(a)), Some(Prefix(b))) => {
                if !a
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
                {
                    return false;
                }
            }
            (Some(a), Some(b)) if a == b => {}
            _ => return false,
        }
    }
}

/// `ERROR_ACCESS_DENIED`.
const ACCESS_DENIED: i32 = 5;

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
    // The same spelling of the path the folders are given in (8.3 names, `\\?\`).
    let root = &fs::normalized(root);
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
    // No position to go on from (no journal could be read when the last one
    // was taken): from here on, but nothing before it was seen.
    if since == 0 {
        return Ok(Replay {
            complete: false,
            next: Some(end),
            ..Replay::default()
        });
    }
    let start = usn_of(since);
    // Another journal, or one that has been cut short past or reset before
    // that position: what happened in between is lost.
    if since >> USN_BITS != tag(id) || start < journal.FirstUsn || start > journal.NextUsn {
        return Ok(gap(end));
    }
    let mut out = Replay::default();
    let (mut at, mut buf) = (start, vec![0u8; 256 * 1024]);
    let mut paths: HashMap<u64, Option<PathBuf>> = HashMap::new();
    let mut slots: HashMap<PathBuf, usize> = HashMap::new();
    let mut complete = true;
    while at < journal.NextUsn {
        if Instant::now() >= deadline || paths.len() >= MAX_FOLDERS {
            complete = false;
            break;
        }
        let Some((next, records, skipped)) = volume.read(id, at, &mut buf)? else {
            // Cut short or made again while it was read: what was found stays,
            // and the rest is a gap.
            let Replay { mut events, .. } = gap(end);
            out.events.append(&mut events);
            return Ok(Replay {
                events: out.events,
                complete: false,
                next: Some(end),
            });
        };
        // Records of a layout that wasn't read are changes that weren't seen.
        if skipped > 0 {
            complete = false;
        }
        let mut stopped_at = None;
        for r in &records {
            let path = match paths.get(&r.parent) {
                Some(p) => p.clone(),
                None => {
                    // What a batch costs is its folders looked up, so the budget
                    // is kept inside it: the next replay goes on from this record.
                    if Instant::now() >= deadline || paths.len() >= MAX_FOLDERS {
                        stopped_at = Some(r.usn);
                        break;
                    }
                    let found = match volume.folder(r.parent) {
                        Ok(p) => p.filter(|p| under(root, p)),
                        // Not this account's to open (Windows' own folders, other
                        // accounts' profiles): nor to list, so nothing in it could
                        // be measured, and its changes are no gap in what can be.
                        Err(e) if e.raw_os_error() == Some(ACCESS_DENIED) => None,
                        Err(_) => {
                            // Not placed: not a change that can be said to be
                            // outside the watched folder.
                            complete = false;
                            None
                        }
                    };
                    paths.insert(r.parent, found.clone());
                    found
                }
            };
            if let Some(path) = path {
                // Each folder once, at its newest position.
                let position = cursor(id, r.usn);
                match slots.get(&path) {
                    Some(&i) => out.events[i].id = position,
                    None => {
                        slots.insert(path.clone(), out.events.len());
                        out.events.push(Event {
                            path,
                            flags: 0,
                            id: position,
                        });
                    }
                }
            }
        }
        if let Some(usn) = stopped_at {
            complete = false;
            // Never back past where this read began: a record whose position
            // isn't past `at` has nothing to resume from.
            at = usn.max(at);
            break;
        }
        if next <= at {
            // Nothing moved: the end.
            break;
        }
        at = next;
    }
    out.complete = complete;
    out.next = Some(cursor(id, at));
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
        let c = cursor(0xDEAD_BEEF_1234, 987_654_320);
        assert_eq!(usn_of(c), 987_654_320);
        assert_eq!(c >> 63, 0, "kept as a signed number");
        // High values survive: a USN of 100 TB, a journal id with every bit set.
        let big = 100 * (1i64 << 40);
        assert_eq!(usn_of(cursor(u64::MAX, big)), big);
        assert_eq!(cursor(u64::MAX, big) >> 63, 0);
        assert_eq!(c >> USN_BITS, tag(0xDEAD_BEEF_1234));
        assert_ne!(tag(0xDEAD_BEEF_1234), tag(0xDEAD_BEEF_1235));
        // A journal id that is only different in its high bits is told apart too.
        assert_ne!(tag(1), tag(1 << 41));
        assert_eq!(usn_of(cursor(5, -3)), 0);
    }

    #[test]
    fn records_are_read_by_their_lengths() {
        let mut buf = 4096i64.to_le_bytes().to_vec();
        buf.extend(record(100, 10, "a.txt"));
        buf.extend(record(200, 20, "a much longer name than before.log"));
        buf.extend(record(300, 30, "z"));
        let (next, rs, skipped) = parse(&buf).unwrap();
        assert_eq!((next, skipped), (4096, 0));
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
        assert_eq!(
            parse(&4096i64.to_le_bytes()).unwrap(),
            (4096, Vec::new(), 0)
        );
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
        let (_, rs, skipped) = parse(&buf).unwrap();
        assert_eq!(rs, [Record { parent: 9, usn: 9 }]);
        assert_eq!(skipped, 1);
    }

    #[test]
    fn paths_are_plain_and_compared_without_case() {
        assert_eq!(
            fs::plain(PathBuf::from(r"\\?\C:\Users\x")),
            PathBuf::from(r"C:\Users\x")
        );
        assert_eq!(
            fs::plain(PathBuf::from(r"\\?\UNC\srv\share\d")),
            PathBuf::from(r"\\srv\share\d")
        );
        assert_eq!(fs::plain(PathBuf::from(r"C:\a")), PathBuf::from(r"C:\a"));
        // A volume with no letter keeps its name: stripped, it would be relative.
        assert_eq!(
            fs::plain(PathBuf::from(r"\\?\Volume{1234}\x")),
            PathBuf::from(r"\\?\Volume{1234}\x")
        );
        assert!(under(Path::new(r"C:\"), Path::new(r"c:\Users\X")));
        assert!(under(Path::new(r"C:\Users"), Path::new(r"c:\Users")));
        // Names are exact: `repo` and `Repo` can be two folders.
        assert!(!under(Path::new(r"C:\Users"), Path::new(r"C:\users")));
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
        // As the journal spells it: the runner's temp folder is given by its 8.3 name.
        let root = fs::normalized(dir.path());
        let volume_root = fs::statfs(&root).unwrap().mount_point;
        if now(&root) == 0 {
            eprintln!("no readable journal on {volume_root}: skipped");
            return;
        }
        let baseline = replay(Path::new(&volume_root), 0, Duration::from_secs(10)).unwrap();
        let Some(saved) = baseline.next else {
            eprintln!("no readable journal on {volume_root}: skipped");
            return;
        };
        // From now on: nothing before it was seen, so not complete.
        assert!(!baseline.complete && baseline.events.is_empty());
        // Nothing has changed under a folder that hasn't been touched.
        let busy = root.join("busy");
        let quiet = root.join("quiet");
        std::fs::create_dir(&quiet).unwrap();
        std::fs::create_dir(&busy).unwrap();
        let first = replay(Path::new(&volume_root), saved, Duration::from_secs(10)).unwrap();
        // (Not asserted complete: other activity on the shared volume may be in
        // folders this account can't place, which makes a replay incomplete.)
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
        let e = replay(Path::new(r"C:\"), 0, Duration::MAX).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    }
}
