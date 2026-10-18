//! FSEvents: which directories changed, from the kernel's change journal.
//!
//! Events are per directory (no per-file events), coalesced over `latency`,
//! and come with persistent IDs: a stream started from a saved ID replays
//! every change since then, across restarts of the watcher and of the Mac.
//! That's what lets the owner sizes be kept up to date by re-listing only
//! the directories that changed, instead of rescanning the disk.

use std::ffi::c_void;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;
use std::{io, ptr};

/// Event flags (`kFSEventStreamEventFlag*`).
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

/// Start from now rather than replaying history.
pub const SINCE_NOW: u64 = u64::MAX;

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

/// A running stream. Events arrive on a background queue and are read with
/// [`Stream::drain`]. Stopped on drop.
pub struct Stream {
    raw: FSEventStreamRef,
    queue: *mut c_void,
    tx: *mut Sender<Event>,
    rx: Receiver<Event>,
}

// The raw handles are only used from methods taking `&mut self` or in drop;
// FSEvents itself is thread-safe.
unsafe impl Send for Stream {}

impl Stream {
    /// Watch `paths` (directories, recursively). `since` is an event ID to
    /// replay from, or [`SINCE_NOW`]. Events caused by this process are
    /// left out, so the watcher's own history writes never wake it.
    pub fn start(paths: &[&Path], since: u64, latency: Duration) -> io::Result<Stream> {
        // Core Foundation needs valid UTF-8, and returns null otherwise.
        let paths: Vec<&str> = paths
            .iter()
            .map(|p| {
                p.to_str().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("FSEvents can't watch a non-UTF-8 path: {}", p.display()),
                    )
                })
            })
            .collect::<io::Result<_>>()?;
        if paths.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no paths to watch",
            ));
        }
        let array = unsafe { cf_paths(&paths) }
            .ok_or_else(|| io::Error::other("could not build the FSEvents path list"))?;
        let (tx, rx) = mpsc::channel();
        let tx = Box::into_raw(Box::new(tx));
        let ctx = FSEventStreamContext {
            version: 0,
            info: tx.cast(),
            retain: None,
            release: None,
            copy_description: None,
        };
        let flags = CREATE_NO_DEFER | CREATE_IGNORE_SELF | CREATE_WATCH_ROOT;
        unsafe {
            let raw = FSEventStreamCreate(
                ptr::null(),
                callback,
                &ctx,
                array,
                since,
                latency.as_secs_f64(),
                flags,
            );
            CFRelease(array);
            if raw.is_null() {
                drop(Box::from_raw(tx));
                return Err(io::Error::other("FSEventStreamCreate failed"));
            }
            let queue = dispatch_queue_create(c"mydiskisfull.fsevents".as_ptr(), ptr::null_mut());
            FSEventStreamSetDispatchQueue(raw, queue);
            if FSEventStreamStart(raw) == 0 {
                FSEventStreamInvalidate(raw);
                FSEventStreamRelease(raw);
                dispatch_release(queue);
                drop(Box::from_raw(tx));
                return Err(io::Error::other("FSEventStreamStart failed"));
            }
            Ok(Stream { raw, queue, tx, rx })
        }
    }

    /// Deliver everything pending now instead of after the latency, and
    /// wait until it has been delivered.
    pub fn flush(&mut self) {
        unsafe { FSEventStreamFlushSync(self.raw) };
    }

    /// Events received so far.
    pub fn drain(&mut self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }

    /// Wait up to `timeout` for the next event.
    pub fn next_timeout(&mut self, timeout: Duration) -> Option<Event> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// The ID of the last event delivered: save it to resume from there.
    pub fn latest_id(&self) -> u64 {
        unsafe { FSEventStreamGetLatestEventId(self.raw) }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            FSEventStreamStop(self.raw);
            FSEventStreamInvalidate(self.raw);
            FSEventStreamRelease(self.raw);
            // Wait for a callback that may still be running on the queue
            // before freeing the sender it uses.
            dispatch_sync_f(self.queue, ptr::null_mut(), noop);
            dispatch_release(self.queue);
            drop(Box::from_raw(self.tx));
        }
    }
}

/// The most recent event ID system-wide.
pub fn current_event_id() -> u64 {
    unsafe { FSEventsGetCurrentEventId() }
}

extern "C" fn callback(
    _stream: *const c_void,
    info: *mut c_void,
    n: usize,
    paths: *mut c_void,
    flags: *const u32,
    ids: *const u64,
) {
    let tx = unsafe { &*(info as *const Sender<Event>) };
    let paths = paths as *const *const libc::c_char;
    for i in 0..n {
        let (path, flags, id) = unsafe {
            let c = std::ffi::CStr::from_ptr(*paths.add(i));
            let path = PathBuf::from(std::ffi::OsString::from_vec(c.to_bytes().to_vec()));
            (path, *flags.add(i), *ids.add(i))
        };
        // The receiver is gone only while the stream is being dropped.
        let _ = tx.send(Event { path, flags, id });
    }
}

extern "C" fn noop(_: *mut c_void) {}

/// A CFArray of CFStrings, or `None` if Core Foundation refuses one.
unsafe fn cf_paths(paths: &[&str]) -> Option<CFArrayRef> {
    unsafe {
        let mut strings: Vec<CFStringRef> = Vec::with_capacity(paths.len());
        for p in paths {
            let s = CFStringCreateWithBytes(
                ptr::null(),
                p.as_ptr(),
                p.len() as isize,
                CF_STRING_ENCODING_UTF8,
                0,
            );
            if s.is_null() {
                break;
            }
            strings.push(s);
        }
        let array = if strings.len() == paths.len() {
            CFArrayCreate(
                ptr::null(),
                strings.as_ptr(),
                strings.len() as isize,
                &kCFTypeArrayCallBacks,
            )
        } else {
            ptr::null()
        };
        // The array holds its own references.
        for s in strings {
            CFRelease(s);
        }
        (!array.is_null()).then_some(array)
    }
}

type FSEventStreamRef = *mut c_void;
type CFArrayRef = *const c_void;
type CFStringRef = *const c_void;
type Callback =
    extern "C" fn(*const c_void, *mut c_void, usize, *mut c_void, *const u32, *const u64);

const CREATE_NO_DEFER: u32 = 0x2;
const CREATE_WATCH_ROOT: u32 = 0x4;
const CREATE_IGNORE_SELF: u32 = 0x8;
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

#[repr(C)]
struct FSEventStreamContext {
    version: isize,
    info: *mut c_void,
    retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<extern "C" fn(*const c_void)>,
    copy_description: Option<extern "C" fn(*const c_void) -> CFStringRef>,
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: *const c_void,
        callback: Callback,
        context: *const FSEventStreamContext,
        paths: CFArrayRef,
        since: u64,
        latency: f64,
        flags: u32,
    ) -> FSEventStreamRef;
    fn FSEventStreamSetDispatchQueue(stream: FSEventStreamRef, queue: *mut c_void);
    fn FSEventStreamStart(stream: FSEventStreamRef) -> u8;
    fn FSEventStreamFlushSync(stream: FSEventStreamRef);
    fn FSEventStreamStop(stream: FSEventStreamRef);
    fn FSEventStreamInvalidate(stream: FSEventStreamRef);
    fn FSEventStreamRelease(stream: FSEventStreamRef);
    fn FSEventStreamGetLatestEventId(stream: FSEventStreamRef) -> u64;
    fn FSEventsGetCurrentEventId() -> u64;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeArrayCallBacks: c_void;
    fn CFStringCreateWithBytes(
        alloc: *const c_void,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        external: u8,
    ) -> CFStringRef;
    fn CFArrayCreate(
        alloc: *const c_void,
        values: *const *const c_void,
        n: isize,
        callbacks: *const c_void,
    ) -> CFArrayRef;
    fn CFRelease(cf: *const c_void);
}

unsafe extern "C" {
    fn dispatch_queue_create(label: *const libc::c_char, attr: *mut c_void) -> *mut c_void;
    fn dispatch_sync_f(queue: *mut c_void, ctx: *mut c_void, f: extern "C" fn(*mut c_void));
    fn dispatch_release(obj: *mut c_void);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real(p: &Path) -> PathBuf {
        std::fs::canonicalize(p).unwrap()
    }

    /// Wait for an event on `dir`, flushing as we go.
    fn wait_for(s: &mut Stream, dir: &Path) -> Vec<Event> {
        let mut seen = Vec::new();
        for _ in 0..50 {
            s.flush();
            seen.extend(s.drain());
            if seen.iter().any(|e| e.path.starts_with(dir)) {
                return seen;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("no event for {} in {seen:?}", dir.display());
    }

    #[test]
    fn reports_the_directory_that_changed_and_replays_history() {
        let tmp = tempfile::tempdir().unwrap();
        let root = real(tmp.path());
        let sub = root.join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        let start = current_event_id();

        // A file written by a child process (our own writes are ignored).
        let child = |dir: &Path| {
            let ok = std::process::Command::new("/usr/bin/touch")
                .arg(dir.join("f"))
                .status()
                .unwrap()
                .success();
            assert!(ok);
        };

        let mut s = Stream::start(&[&root], SINCE_NOW, Duration::from_millis(50)).unwrap();
        child(&sub);
        let seen = wait_for(&mut s, &sub);
        assert!(seen.iter().any(|e| e.path.starts_with(&sub)), "{seen:?}");
        drop(s);

        // Replaying from before the write sees it again, then says done.
        let mut s = Stream::start(&[&root], start, Duration::from_millis(50)).unwrap();
        let mut replay = Vec::new();
        while let Some(e) = s.next_timeout(Duration::from_secs(5)) {
            let done = e.flags & flag::HISTORY_DONE != 0;
            replay.push(e);
            if done {
                break;
            }
        }
        assert!(
            replay.iter().any(|e| e.path.starts_with(&sub)),
            "{replay:?}"
        );
        assert!(replay.last().unwrap().flags & flag::HISTORY_DONE != 0);
        assert!(s.latest_id() >= start);
    }

    #[test]
    fn invalid_paths_are_errors_not_crashes() {
        use std::os::unix::ffi::OsStrExt;
        let bad = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff\xfe"));
        let err = Stream::start(&[bad], SINCE_NOW, Duration::from_millis(50)).err();
        assert_eq!(err.map(|e| e.kind()), Some(io::ErrorKind::InvalidInput));
        let err = Stream::start(&[], SINCE_NOW, Duration::from_millis(50)).err();
        assert_eq!(err.map(|e| e.kind()), Some(io::ErrorKind::InvalidInput));
    }

    #[test]
    fn own_writes_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let root = real(tmp.path());
        let mut s = Stream::start(&[&root], SINCE_NOW, Duration::from_millis(50)).unwrap();
        std::fs::write(root.join("mine"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        s.flush();
        let seen: Vec<Event> = s
            .drain()
            .into_iter()
            .filter(|e| e.flags & flag::HISTORY_DONE == 0)
            .collect();
        assert!(seen.is_empty(), "{seen:?}");
    }
}
