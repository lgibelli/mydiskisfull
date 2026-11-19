//! What changed on a volume since a saved position: the neutral face of the
//! file-system change journal, so the watcher doesn't care that this one is
//! FSEvents.
//!
//! A position is an opaque, increasing `u64`: save the largest event ID
//! seen and pass it to the next [`replay`]. [`Event`] and [`flag`] are
//! FSEvents' own.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::fsevents::{self, Stream};
pub use crate::fsevents::{Event, flag};

/// How long FSEvents may coalesce events before delivering them.
const LATENCY: Duration = Duration::from_millis(50);

/// What a replay found.
#[derive(Debug, Default)]
pub struct Replay {
    /// The directories that changed since the position, oldest first.
    pub events: Vec<Event>,
    /// Whether the journal said it had delivered all of its history. When
    /// false (`timeout` ran out first) there may be more: a gap.
    pub complete: bool,
}

/// The position of "now": where a first replay starts from.
pub fn now() -> u64 {
    fsevents::current_event_id()
}

/// Replay the journal under `root` from position `since` to its end,
/// waiting at most `timeout`. Changes made by this process are left out.
pub fn replay(root: &Path, since: u64, timeout: Duration) -> io::Result<Replay> {
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the replay timeout is too long",
        )
    })?;
    let mut stream = Stream::start(&[root], since, LATENCY)?;
    let mut out = Replay::default();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match stream.next_timeout(left) {
            Some(e) if e.flags & flag::HISTORY_DONE != 0 => {
                out.complete = true;
                break;
            }
            Some(e) => out.events.push(e),
            None => break,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unrepresentable_timeout_is_an_error_not_a_panic() {
        let e = replay(Path::new("/"), now(), Duration::MAX).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    }
}
