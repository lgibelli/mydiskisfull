//! What changed on a volume since a saved position: the neutral face of the
//! file-system change journal.
//!
//! Not read yet. Until it is, a replay finds nothing and says it isn't
//! complete, which the watcher counts as a gap, and so keeps sampling.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
}

/// The position of "now": where a first replay starts from.
pub fn now() -> u64 {
    0
}

/// Replay the journal under `root` from position `since`. Not written yet:
/// nothing, and not complete.
pub fn replay(_root: &Path, _since: u64, _timeout: Duration) -> io::Result<Replay> {
    Ok(Replay::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn until_the_journal_is_read_every_replay_is_a_gap() {
        let r = replay(Path::new(r"C:\"), now(), Duration::from_secs(1)).unwrap();
        assert!(r.events.is_empty() && !r.complete);
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
}
