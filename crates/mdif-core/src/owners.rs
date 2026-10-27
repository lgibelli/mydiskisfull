//! Owners: the folders disk space belongs to, and how fast each one grows.
//!
//! An owner is a folder at a level that means something to a person: an
//! app's container, cache or support folder, a project and, separately, its
//! build output or one of its worktrees, a top-level folder of the home
//! ([`Rules`]).
//!
//! Growth is tracked incrementally, with no scan up front. Each period the
//! watcher replays the FSEvents journal since the previous period, re-lists
//! only the directories that changed (one level, not recursively), and turns
//! the change in each one's direct-file size into growth of its owner
//! ([`Tracker`]). A directory seen for the first time has no previous size,
//! so only its files created since the previous period count: the first
//! period after a start is a lower bound, later ones are exact.
//!
//! FSEvents doesn't always say when a file grows: a 28 GB download by a
//! torrent client showed only its folder being created, and the folder was
//! listed once, 10 GB in. So a folder that grew is listed again the next
//! period, event or not, until it holds still.
//!
//! A folder moved as a whole (a rename, a move to the Trash) changes only
//! its parents. When a parent is listed and a folder known below it is gone,
//! it's looked up by its inode number (`fsgetpath`): what was known of it
//! moves to where it is now, to its new owner (the Trash, until it's
//! emptied), or is freed if it was deleted.
//!
//! Sizes are allocated bytes, which snapshots don't change, divided among
//! hard links and among pure clones so neither is counted twice.
//!
//! Listing costs about 3.5 µs per entry whatever is asked for (10–15 µs at
//! background priority), so the few busy folders with tens of thousands of
//! entries (a browser cache, a build's `deps`) would eat the watcher's
//! budget if they were re-listed every period. So a changed folder waits:
//! - one period per [`DEFER_UNIT`] entries (up to [`MAX_DEFER`]);
//! - twice as long once it is found [`STEADY`] (changed little since it was
//!   last listed), four times as long if it stays so: caches and logs churn
//!   but keep their size, while a filler keeps growing and is listed often;
//! - longer still when the period has listed [`PERIOD_ENTRIES`] already,
//!   the most overdue folders going first.
//!
//! Whatever a folder grew while it waited counts in full when it is listed,
//! a little later. (Unless the watcher restarts meanwhile: then only files
//! created since the previous period count, as for any folder seen first.)

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::io;
use std::ops::Bound;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use mdif_macos::bulk::{self, Detail, EF_MAY_SHARE_BLOCKS, Entry, Kind};
use mdif_macos::fs::FsId;
use mdif_macos::fsevents::{Event, flag};

use crate::privacy;

/// Folder names that hold regenerable build output inside a project.
pub const ARTIFACTS: &[&str] = &[
    "target",
    "node_modules",
    "build",
    ".dart_tool",
    ".gradle",
    ".next",
    ".turbo",
    "dist",
    "Pods",
    "DerivedData",
    ".build",
    ".venv",
];

/// Home folders whose children are projects.
pub const CODE_ROOTS: &[&str] = &[
    "devel",
    "dev",
    "Developer",
    "src",
    "code",
    "Code",
    "Projects",
    "projects",
    "git",
    "repos",
    "work",
];

/// Owner growth is reported once it adds up to this much, either way; the
/// rest is carried to later periods so slow growth isn't lost.
pub const MIN_GROWTH: i64 = 1_000_000;

/// A folder with this many entries is re-listed at most every other
/// period, twice as many every third, and so on.
pub const DEFER_UNIT: u32 = 1024;
/// The longest a changed folder that keeps growing waits to be re-listed,
/// in periods.
pub const MAX_DEFER: u32 = 4;
/// A folder whose size changed by less than this per period since it was
/// last listed is steady, and waits twice as long; then four times
/// ([`MAX_STEADY`]). The default: a filler worth an incident grows at least
/// five times faster (1 GB in 6 hours is 83 MB per 30-minute period). The
/// watcher derives it from its incident rules ([`Tracker::steady_below`]).
pub const STEADY: u64 = 16_000_000;
/// Doublings of the wait for a steady folder: 16 periods (8 hours) at most.
pub const MAX_STEADY: u32 = 2;
/// Entries listed per period, beyond which folders that have been listed
/// before wait for the next one (about 0.3 s at background priority).
/// Folders seen for the first time are listed regardless.
pub const PERIOD_ENTRIES: usize = 20_000;

/// Directories re-listed per period at most when FSEvents asks for a whole
/// subtree to be rescanned (it does after dropping events).
const MAX_RESCAN: usize = 20_000;

#[derive(Debug, Clone, PartialEq)]
enum Depth {
    /// This many components below the prefix.
    Fixed(usize),
    /// A project, its artifacts and worktrees ([`ARTIFACTS`]).
    Projects,
}

/// Which folder owns a path: the longest matching prefix decides how many
/// components below it make the owner.
#[derive(Debug, Clone)]
pub struct Rules {
    rules: Vec<(PathBuf, Depth)>,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            rules: vec![(PathBuf::from("/"), Depth::Fixed(1))],
        }
    }
}

impl Rules {
    /// The rules for a Mac with `home` as the user's home folder.
    pub fn for_home(home: &Path) -> Rules {
        let mut r = Rules::default()
            .depth("/Users", 1)
            .depth("/Applications", 1)
            .depth("/Library", 1)
            .depth("/Library/Application Support", 1)
            .depth("/Library/Caches", 1)
            .depth("/Library/Developer", 2)
            .depth("/opt/homebrew", 1)
            .depth("/usr/local", 1)
            .depth("/private", 1)
            .depth("/private/var", 1)
            .depth("/private/var/db", 1)
            // vc/<id>/{T,C,X}/<app>: whose temporary files.
            .depth("/private/var/folders", 4)
            .depth(home, 1)
            .depth(home.join("Library"), 1)
            .depth(home.join(".cache"), 1)
            .depth(home.join(".Trash"), 0);
        for lib in [
            "Application Support",
            "Caches",
            "Containers",
            "Group Containers",
            "CloudStorage",
            "Mobile Documents",
            "Logs",
        ] {
            r = r.depth(home.join("Library").join(lib), 1);
        }
        r = r.depth(home.join("Library/Developer"), 2);
        for media in ["Movies", "Music", "Pictures", "Documents", "Desktop"] {
            r = r.depth(home.join(media), 1);
        }
        for code in CODE_ROOTS {
            r = r.projects(home.join(code));
        }
        r
    }

    /// Owners are `depth` components below `prefix`.
    pub fn depth(mut self, prefix: impl Into<PathBuf>, depth: usize) -> Rules {
        self.set(prefix.into(), Depth::Fixed(depth));
        self
    }

    /// Children of `prefix` are projects.
    pub fn projects(mut self, prefix: impl Into<PathBuf>) -> Rules {
        self.set(prefix.into(), Depth::Projects);
        self
    }

    fn set(&mut self, prefix: PathBuf, depth: Depth) {
        self.rules.retain(|(p, _)| *p != prefix);
        self.rules.push((prefix, depth));
    }

    pub fn owner_of(&self, path: &Path) -> PathBuf {
        let (prefix, depth) = self
            .rules
            .iter()
            .filter(|(p, _)| path.starts_with(p))
            .max_by_key(|(p, _)| p.components().count())
            .expect("the root rule matches every absolute path");
        let rest: Vec<&OsStr> = path
            .strip_prefix(prefix)
            .map(|r| r.iter().collect())
            .unwrap_or_default();
        let take = match depth {
            Depth::Fixed(n) => (*n).min(rest.len()),
            Depth::Projects => project_depth(&rest),
        };
        let mut owner = prefix.clone();
        owner.extend(&rest[..take]);
        owner
    }
}

/// `proj`, `proj/.claude/worktrees/<name>`, or `proj/…/<artifact>`.
fn project_depth(rest: &[&OsStr]) -> usize {
    if rest.is_empty() {
        return 0;
    }
    if rest.len() >= 4 && rest[1] == ".claude" && rest[2] == "worktrees" {
        return 4;
    }
    rest[1..]
        .iter()
        .position(|c| c.to_str().is_some_and(|c| ARTIFACTS.contains(&c)))
        .map_or(1, |i| i + 2)
}

/// Bytes a directory entry adds to its folder: allocated (stable when a
/// snapshot is taken or released, unlike private size), split among hard
/// links and pure clones. Cloud placeholders and folders add nothing.
pub fn entry_size(e: &Entry) -> u64 {
    if e.kind == Kind::Dir || e.is_dataless() {
        return 0;
    }
    let mut share = e.nlink.max(1) as u64;
    if e.ext_flags & EF_MAY_SHARE_BLOCKS != 0 {
        share *= e.clone_refcnt.max(1) as u64;
    }
    e.allocated / share
}

#[derive(Debug, Clone, Copy)]
struct Dir {
    /// Bytes of the files directly in it.
    own: u64,
    /// Entries at the last listing, which is what listing it costs.
    entries: u32,
    /// When it was last listed (Unix seconds, wall clock).
    seen: i64,
    /// Listings in a row that found it [`STEADY`], up to [`MAX_STEADY`].
    steady: u32,
    /// Its inode number, to find it if it moves (0: unknown).
    inode: u64,
}

impl Dir {
    /// Periods to wait between listings.
    fn defer(&self) -> u32 {
        (self.entries / DEFER_UNIT).clamp(1, MAX_DEFER) << self.steady
    }
}

/// What one period found.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Update {
    /// Growth per owner (negative: freed), largest change first, for owners
    /// whose change since they were last reported reached [`MIN_GROWTH`].
    pub growth: Vec<(PathBuf, i64)>,
    /// Directories re-listed.
    pub listed: usize,
    /// Entries in them: listing costs about 3.5 µs each, so this is the
    /// period's cost.
    pub entries: usize,
    /// The directories with the most entries, most first (at most 5).
    pub biggest: Vec<(PathBuf, usize)>,
    /// Changed directories left for a later period: too big or too steady
    /// to list often, or the period's [`PERIOD_ENTRIES`] were spent.
    pub deferred: usize,
    pub unreadable: usize,
    /// Directories left alone because opening them would show a privacy
    /// prompt (see [`crate::privacy`]).
    pub skipped: usize,
    /// Whole-volume rescans FSEvents asked for (history lost, e.g. after
    /// its journal was reset), which are skipped.
    pub gaps: usize,
}

/// Directory sizes as last seen, and growth not yet reported.
pub struct Tracker {
    rules: Rules,
    /// Device of the watched volume; directories elsewhere are ignored.
    dev: Option<i32>,
    /// Its file-system ID, to follow folders that move ([`Tracker::follow_moves`]).
    fsid: Option<FsId>,
    dirs: BTreeMap<PathBuf, Dir>,
    carry: HashMap<PathBuf, i64>,
    /// Changed but deferred.
    pending: BTreeSet<PathBuf>,
    /// Seconds per period, the unit of deferral.
    period: i64,
    /// Entries a period lists before folders seen before wait.
    budget: usize,
    /// Bytes per period a steady folder changes by less than.
    steady: u64,
}

impl Tracker {
    /// `period` is how often [`Tracker::apply`] is called, in seconds.
    pub fn new(rules: Rules, dev: Option<i32>, period: i64) -> Self {
        Tracker {
            rules,
            dev,
            fsid: None,
            dirs: BTreeMap::new(),
            carry: HashMap::new(),
            pending: BTreeSet::new(),
            period,
            budget: PERIOD_ENTRIES,
            steady: STEADY,
        }
    }

    /// Folders whose size changes by less than `bytes` per period are
    /// steady ([`STEADY`] by default).
    pub fn steady_below(&mut self, bytes: u64) {
        self.steady = bytes;
    }

    /// Follow folders that move on the volume with this ID; without it,
    /// one that's gone from its parent is taken as deleted.
    pub fn follow_moves(&mut self, fsid: FsId) {
        self.fsid = Some(fsid);
    }

    pub fn rules(&self) -> &Rules {
        &self.rules
    }

    pub fn dirs_tracked(&self) -> usize {
        self.dirs.len()
    }

    /// Re-list the directories in `events` and return the growth per owner.
    /// `created_after` is the wall-clock time of the previous period: files
    /// in a directory seen for the first time count if created after it.
    /// `now` is on the caller's clock, in the unit of the period.
    pub fn apply(&mut self, events: &[Event], created_after: i64, now: i64) -> Update {
        let mut up = Update::default();
        // Each directory once; `true` when its whole subtree must be rescanned.
        let mut dirty: BTreeMap<PathBuf, bool> = BTreeMap::new();
        for e in events {
            if e.flags & (flag::HISTORY_DONE | flag::MOUNT | flag::UNMOUNT) != 0 {
                continue;
            }
            let dir: PathBuf = e.path.components().collect();
            if e.must_rescan() {
                if dir.components().count() <= 2 {
                    up.gaps += 1;
                } else {
                    dirty.insert(dir, true);
                }
            } else {
                dirty.entry(dir).or_insert(false);
            }
        }
        for dir in std::mem::take(&mut self.pending) {
            dirty.entry(dir).or_insert(false);
        }
        let mut deltas: HashMap<PathBuf, i64> = HashMap::new();
        let mut budget = MAX_RESCAN;
        // Folders listed before that are due, and how overdue (1: just due).
        let mut due: Vec<(f64, PathBuf)> = Vec::new();
        for (dir, recursive) in dirty {
            let (wait, seen) = match self.dirs.get(&dir) {
                Some(d) if !recursive => (d.defer() as i64 * self.period, d.seen),
                _ => {
                    self.measure(
                        dir,
                        recursive,
                        created_after,
                        now,
                        &mut budget,
                        &mut deltas,
                        &mut up,
                    );
                    continue;
                }
            };
            let waited = now - seen;
            // Listed recently: later. (The slack absorbs periods that run a
            // little early.)
            if waited < wait - self.period / 10 {
                up.deferred += 1;
                self.pending.insert(dir);
            } else {
                due.push((waited as f64 / wait as f64, dir));
            }
        }
        due.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (_, dir) in due {
            if up.entries >= self.budget {
                up.deferred += 1;
                self.pending.insert(dir);
                continue;
            }
            self.measure(
                dir,
                false,
                created_after,
                now,
                &mut budget,
                &mut deltas,
                &mut up,
            );
        }
        for (owner, d) in deltas {
            *self.carry.entry(owner).or_default() += d;
        }
        self.carry.retain(|owner, d| {
            if d.abs() >= MIN_GROWTH {
                up.growth.push((owner.clone(), *d));
                false
            } else {
                *d != 0
            }
        });
        up.growth
            .sort_by_key(|(o, d)| (Reverse(d.abs()), o.clone()));
        up.biggest.sort_by_key(|(d, n)| (Reverse(*n), d.clone()));
        up.biggest.truncate(5);
        up
    }

    #[allow(clippy::too_many_arguments)]
    fn measure(
        &mut self,
        dir: PathBuf,
        recursive: bool,
        created_after: i64,
        now: i64,
        budget: &mut usize,
        deltas: &mut HashMap<PathBuf, i64>,
        up: &mut Update,
    ) {
        let mut stack = vec![dir];
        while let Some(dir) = stack.pop() {
            if !privacy::may_open(&dir) {
                up.skipped += 1;
                continue;
            }
            match bulk::read_dir_with(&dir, Detail::Sizes) {
                Ok(entries) => {
                    // A mount point lists the other volume.
                    if let (Some(dev), Some(first)) = (self.dev, entries.first())
                        && first.dev != dev
                    {
                        continue;
                    }
                    up.listed += 1;
                    up.entries += entries.len();
                    if entries.len() >= DEFER_UNIT as usize {
                        up.biggest.push((dir.clone(), entries.len()));
                    }
                    // Folders known below it that aren't in it any more:
                    // moved or deleted.
                    let present: HashSet<&OsStr> = entries
                        .iter()
                        .filter(|e| e.kind == Kind::Dir)
                        .map(|e| e.name.as_os_str())
                        .collect();
                    for child in self.known_children(&dir) {
                        if child.file_name().is_some_and(|n| !present.contains(n)) {
                            self.gone(&child, deltas);
                        }
                    }
                    let (mut own, mut created) = (0u64, 0u64);
                    for e in &entries {
                        if e.kind == Kind::Dir {
                            if recursive && !e.is_dataless() && *budget > 0 {
                                *budget -= 1;
                                stack.push(dir.join(&e.name));
                            }
                            continue;
                        }
                        let size = entry_size(e);
                        own += size;
                        if e.crtime > created_after {
                            created += size;
                        }
                    }
                    let mut state = Dir {
                        own,
                        entries: entries.len().min(u32::MAX as usize) as u32,
                        seen: now,
                        steady: 0,
                        inode: std::fs::symlink_metadata(&dir).map_or(0, |m| m.ino()),
                    };
                    let before = self.dirs.get(&dir).copied();
                    if let Some(b) = before {
                        let periods = ((now - b.seen + self.period / 2) / self.period).max(1);
                        if own.abs_diff(b.own) < self.steady * periods as u64 {
                            state.steady = (b.steady + 1).min(MAX_STEADY);
                        }
                    }
                    self.dirs.insert(dir.clone(), state);
                    let delta = match before {
                        Some(b) => own as i64 - b.own as i64,
                        None => created as i64,
                    };
                    if delta != 0 {
                        *deltas.entry(self.rules.owner_of(&dir)).or_default() += delta;
                    }
                    // Growing: listed again next period, whatever FSEvents says.
                    if delta > 0 && delta >= self.steady as i64 {
                        self.pending.insert(dir.clone());
                    }
                }
                Err(e) if is_gone(&e) => self.gone(&dir, deltas),
                Err(_) => up.unreadable += 1,
            }
        }
    }

    /// The children of `dir` that it, or folders known below them, are
    /// known by.
    fn known_children(&self, dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut from = Bound::Excluded(dir.to_path_buf());
        while let Some((p, _)) = self.dirs.range((from, Bound::Unbounded)).next() {
            let Some(name) = p.strip_prefix(dir).ok().and_then(|r| r.iter().next()) else {
                break;
            };
            let child = dir.join(name);
            // Past all that's below it: paths compare by component, and no
            // name sorts after 0xFF, which UTF-8 never holds.
            from = Bound::Excluded(child.join(OsStr::from_bytes(b"\xff")));
            out.push(child);
        }
        out
    }

    /// `root` isn't where it was. What was known of it and below it moves to
    /// where it is now, and to that place's owner, or is freed if it was
    /// deleted (or can't be followed).
    fn gone(&mut self, root: &Path, deltas: &mut HashMap<PathBuf, i64>) {
        let below: Vec<PathBuf> = self
            .dirs
            .range(root.to_path_buf()..)
            .take_while(|(p, _)| p.starts_with(root))
            .map(|(p, _)| p.clone())
            .collect();
        let to = self.now_at(root, &below);
        if to.as_deref() == Some(root) {
            return;
        }
        for p in below {
            let d = self.dirs.remove(&p).expect("just listed");
            let pending = self.pending.remove(&p);
            *deltas.entry(self.rules.owner_of(&p)).or_default() -= d.own as i64;
            let Some(to) = &to else { continue };
            let rel = p.strip_prefix(root).expect("below it");
            let moved = if rel.as_os_str().is_empty() {
                to.clone()
            } else {
                to.join(rel)
            };
            if !self.dirs.contains_key(&moved) {
                *deltas.entry(self.rules.owner_of(&moved)).or_default() += d.own as i64;
                if pending {
                    self.pending.insert(moved.clone());
                }
                self.dirs.insert(moved, d);
            }
        }
    }

    /// Where `root` is now, from the inode number of a folder known in it
    /// (`below`): `None` if it was deleted or can't be looked up (nothing
    /// is looked up while privacy prompts are avoided: it may be in a
    /// guarded folder now).
    fn now_at(&self, root: &Path, below: &[PathBuf]) -> Option<PathBuf> {
        let fsid = self.fsid.filter(|_| !privacy::avoiding())?;
        below.iter().find_map(|p| {
            let d = self.dirs.get(p).filter(|d| d.inode != 0)?;
            let now = mdif_macos::fs::path_of(fsid, d.inode).ok()?;
            // Its place inside what moved is the same.
            let rel = p.strip_prefix(root).ok()?;
            let depth = rel.components().count();
            (depth == 0 || now.ends_with(rel))
                .then(|| now.ancestors().nth(depth).map(Path::to_path_buf))
                .flatten()
        })
    }

    /// Forget directories not listed since `before`, to bound memory. If
    /// they change again they count as seen for the first time.
    pub fn prune(&mut self, before: i64) {
        let pending = &self.pending;
        self.dirs
            .retain(|p, d| d.seen >= before || pending.contains(p));
    }
}

/// A cleanup of an owner, and how much has come back since.
#[derive(Debug, Clone, PartialEq)]
pub struct Regrowth {
    pub owner: String,
    /// When it shrank by at least the threshold in one period.
    pub at: i64,
    pub freed: u64,
    /// Net growth since then.
    pub back: i64,
}

impl Regrowth {
    /// Share of what was freed that has come back.
    pub fn share(&self) -> f64 {
        self.back.max(0) as f64 / self.freed.max(1) as f64
    }
}

/// The most recent cleanup of each owner (a drop of at least `min` bytes in
/// one period) in `series` (`owner -> [(ts, bytes)]`), most freed first.
/// Clearing something that comes straight back isn't worth doing by hand;
/// that's what this tells.
pub fn regrowth(series: &HashMap<String, Vec<(i64, i64)>>, min: u64) -> Vec<Regrowth> {
    let mut out: Vec<Regrowth> = series
        .iter()
        .filter_map(|(owner, rows)| {
            let i = rows.iter().rposition(|(_, b)| *b <= -(min as i64))?;
            Some(Regrowth {
                owner: owner.clone(),
                at: rows[i].0,
                freed: rows[i].1.unsigned_abs(),
                back: rows[i + 1..].iter().map(|(_, b)| b).sum(),
            })
        })
        .collect();
    out.sort_by_key(|r| (Reverse(r.freed), r.owner.clone()));
    out
}

fn is_gone(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const MB: u64 = 1_000_000;

    fn rules() -> Rules {
        Rules::for_home(Path::new("/Users/me"))
    }

    fn owner(p: &str) -> String {
        rules().owner_of(Path::new(p)).display().to_string()
    }

    #[test]
    fn owners_are_folders_a_person_recognises() {
        assert_eq!(
            owner("/Users/me/Library/Caches/com.spotify.client/Data/x"),
            "/Users/me/Library/Caches/com.spotify.client"
        );
        assert_eq!(
            owner("/Users/me/Library/Containers/net.whatsapp.WhatsApp/Data/tmp"),
            "/Users/me/Library/Containers/net.whatsapp.WhatsApp"
        );
        assert_eq!(
            owner("/Users/me/Library/Developer/Xcode/DerivedData/App-abc/Build"),
            "/Users/me/Library/Developer/Xcode/DerivedData"
        );
        assert_eq!(
            owner("/Users/me/Library/Messages/Attachments/0a"),
            "/Users/me/Library/Messages"
        );
        assert_eq!(owner("/Users/me/Downloads/big.iso"), "/Users/me/Downloads");
        assert_eq!(owner("/Users/me/.Trash/old"), "/Users/me/.Trash");
        assert_eq!(owner("/Users/me/.cargo/registry/cache"), "/Users/me/.cargo");
        assert_eq!(owner("/Users/me"), "/Users/me");
        assert_eq!(
            owner("/Applications/Xcode.app/Contents"),
            "/Applications/Xcode.app"
        );
        assert_eq!(
            owner("/private/var/folders/vc/abc123/T/com.apple.x/y"),
            "/private/var/folders/vc/abc123/T/com.apple.x"
        );
        assert_eq!(
            owner("/opt/homebrew/Cellar/llvm/19"),
            "/opt/homebrew/Cellar"
        );
        assert_eq!(owner("/Volumes/USB/stuff"), "/Volumes");
    }

    #[test]
    fn projects_split_out_build_output_and_worktrees() {
        assert_eq!(
            owner("/Users/me/devel/app/src/main.rs"),
            "/Users/me/devel/app"
        );
        assert_eq!(
            owner("/Users/me/devel/app/target/debug/deps"),
            "/Users/me/devel/app/target"
        );
        assert_eq!(
            owner("/Users/me/devel/web/apps/site/node_modules/react"),
            "/Users/me/devel/web/apps/site/node_modules"
        );
        assert_eq!(
            owner("/Users/me/devel/app/.claude/worktrees/fix-1/build/ios"),
            "/Users/me/devel/app/.claude/worktrees/fix-1"
        );
        assert_eq!(owner("/Users/me/devel"), "/Users/me/devel");
        // Components, never substrings.
        assert_eq!(
            owner("/Users/me/devel/app/targets/x"),
            "/Users/me/devel/app"
        );
    }

    fn write(p: &Path, len: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![3u8; len]).unwrap();
        fs::File::open(p).unwrap().sync_all().unwrap();
    }

    fn event(p: &Path) -> Event {
        Event {
            path: p.to_path_buf(),
            flags: 0,
            id: 1,
        }
    }

    fn growth(up: &Update, owner: &Path) -> i64 {
        up.growth
            .iter()
            .find(|(o, _)| o == owner)
            .map_or(0, |(_, d)| *d)
    }

    #[test]
    fn growth_is_tracked_per_owner_from_changed_dirs_only() {
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        let rules = Rules::default().depth(&root, 1);
        let mut tr = Tracker::new(rules, None, 1);
        let (logs, cache) = (root.join("app/logs"), root.join("cache/blobs"));
        write(&logs.join("old.log"), 5 * MB as usize);

        // First sight: only files created since the last period count.
        let before = 0; // everything counts as new
        write(&cache.join("a.bin"), 3 * MB as usize);
        let up = tr.apply(&[event(&logs), event(&cache)], before, 100);
        assert!(growth(&up, &root.join("app")) >= 5 * MB as i64);
        assert!(growth(&up, &root.join("cache")) >= 3 * MB as i64);

        // Later periods are exact deltas, and unchanged dirs aren't listed.
        write(&logs.join("new.log"), 2 * MB as usize);
        fs::remove_file(cache.join("a.bin")).unwrap();
        let up = tr.apply(&[event(&logs), event(&cache)], i64::MAX, 200);
        assert_eq!(up.listed, 2);
        let app = growth(&up, &root.join("app"));
        assert!((2 * MB as i64..3 * MB as i64).contains(&app), "{up:?}");
        let cache_d = growth(&up, &root.join("cache"));
        assert!(cache_d <= -(3 * MB as i64), "{up:?}");

        // A deleted folder frees what we knew of it and below it.
        fs::remove_dir_all(root.join("app")).unwrap();
        let up = tr.apply(&[event(&logs)], i64::MAX, 300);
        assert!(growth(&up, &root.join("app")) <= -(7 * MB as i64), "{up:?}");
        assert_eq!(tr.dirs_tracked(), 1);
    }

    #[test]
    fn a_folder_that_grew_is_listed_again_without_an_event() {
        // A download written while kept open: FSEvents may report its
        // folder once, when it's created.
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        let film = root.join("Storage/film");
        let file = film.join("film.mkv");
        write(&file, 20 * MB as usize);
        let mut tr = Tracker::new(Rules::default().depth(&root, 1), None, 1);
        let up = tr.apply(&[event(&film)], 0, 100);
        assert!(
            growth(&up, &root.join("Storage")) >= 20 * MB as i64,
            "{up:?}"
        );

        let mut f = fs::OpenOptions::new().append(true).open(&file).unwrap();
        std::io::Write::write_all(&mut f, &vec![3u8; 30 * MB as usize]).unwrap();
        f.sync_all().unwrap();
        let up = tr.apply(&[], i64::MAX, 200);
        assert_eq!(up.listed, 1, "{up:?}");
        assert!(
            growth(&up, &root.join("Storage")) >= 30 * MB as i64,
            "{up:?}"
        );

        // Found still: no more listings without an event.
        let up = tr.apply(&[], i64::MAX, 300);
        assert!(up.growth.is_empty(), "{up:?}");
        let up = tr.apply(&[], i64::MAX, 400);
        assert_eq!(up.listed, 0, "{up:?}");
    }

    #[test]
    fn a_folder_moved_to_the_trash_takes_its_size_along_until_emptied() {
        let _on = crate::privacy::SWITCH
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        let (storage, trash) = (root.join("Desktop/Storage"), root.join(".Trash"));
        let film = storage.join("film");
        write(&film.join("sub/film.mkv"), 20 * MB as usize);
        fs::create_dir_all(&trash).unwrap();
        let rules = Rules::default()
            .depth(&root, 1)
            .depth(root.join("Desktop"), 1);
        let mut tr = Tracker::new(rules.clone(), None, 1);
        tr.follow_moves(mdif_macos::fs::fsid(&root).unwrap());
        let up = tr.apply(&[event(&film.join("sub"))], 0, 100);
        assert!(growth(&up, &storage) >= 20 * MB as i64, "{up:?}");

        // Moved: only its parents change.
        fs::rename(&film, trash.join("film")).unwrap();
        let up = tr.apply(&[event(&storage), event(&trash)], i64::MAX, 200);
        assert!(growth(&up, &storage) <= -(20 * MB as i64), "{up:?}");
        assert!(growth(&up, &trash) >= 20 * MB as i64, "{up:?}");

        // Emptied: freed from the Trash.
        fs::remove_dir_all(trash.join("film")).unwrap();
        let up = tr.apply(&[event(&trash)], i64::MAX, 300);
        assert!(growth(&up, &trash) <= -(20 * MB as i64), "{up:?}");
        assert!(!tr.dirs.keys().any(|p| p.starts_with(trash.join("film"))));

        // Not followed: gone from where it was.
        let mut tr = Tracker::new(rules, None, 1);
        write(&storage.join("other/x.bin"), 20 * MB as usize);
        tr.apply(&[event(&storage.join("other"))], 0, 100);
        fs::rename(storage.join("other"), trash.join("other")).unwrap();
        let up = tr.apply(&[event(&storage), event(&trash)], i64::MAX, 200);
        assert!(growth(&up, &storage) <= -(20 * MB as i64), "{up:?}");
        assert_eq!(growth(&up, &trash), 0, "{up:?}");
    }

    #[test]
    fn first_sight_ignores_files_older_than_the_last_period() {
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        let dir = root.join("x");
        write(&dir.join("old.bin"), 4 * MB as usize);
        let mut tr = Tracker::new(Rules::default().depth(&root, 1), None, 1);
        let up = tr.apply(&[event(&dir)], i64::MAX, 100);
        assert!(up.growth.is_empty(), "{up:?}");
        assert_eq!(up.listed, 1);
    }

    #[test]
    fn small_changes_are_carried_until_they_add_up() {
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        let dir = root.join("slow");
        fs::create_dir(&dir).unwrap();
        let mut tr = Tracker::new(Rules::default().depth(&root, 1), None, 1);
        tr.apply(&[event(&dir)], i64::MAX, 0);
        let mut reported = 0;
        for i in 0..5 {
            write(&dir.join(format!("{i}")), 300_000);
            let up = tr.apply(&[event(&dir)], i64::MAX, i + 1);
            reported += growth(&up, &dir);
        }
        // It is steady, so it may still be waiting.
        reported += growth(&tr.apply(&[], i64::MAX, 100), &dir);
        assert!(reported >= MIN_GROWTH, "1.5 MB in 300 KB steps: {reported}");
    }

    #[test]
    fn rescans_walk_the_subtree_and_hard_links_count_once() {
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        let a = root.join("p/a/deep/data.bin");
        write(&a, 6 * MB as usize);
        fs::hard_link(&a, root.join("p/a/link.bin")).unwrap();
        let mut tr = Tracker::new(Rules::default().depth(&root, 1), None, 1);
        let rescan = Event {
            path: root.join("p"),
            flags: flag::MUST_SCAN_SUBDIRS,
            id: 1,
        };
        let up = tr.apply(&[rescan], 0, 1);
        assert_eq!(up.listed, 3);
        let g = growth(&up, &root.join("p"));
        assert!((6 * MB as i64..7 * MB as i64).contains(&g), "{up:?}");
    }

    #[test]
    fn big_busy_folders_are_listed_less_often_but_counted_in_full() {
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        let dir = root.join("cache");
        fs::create_dir(&dir).unwrap();
        for i in 0..(2 * DEFER_UNIT) {
            fs::write(dir.join(i.to_string()), b"").unwrap();
        }
        let period = 1800;
        let mut tr = Tracker::new(Rules::default().depth(&root, 1), None, period);
        tr.apply(&[event(&dir)], i64::MAX, 0);
        // Changed again one period later: it has 2048 entries, so it waits
        // for a second period.
        write(&dir.join("big.bin"), 3 * MB as usize);
        let up = tr.apply(&[event(&dir)], i64::MAX, period);
        assert_eq!((up.listed, up.deferred), (0, 1));
        assert!(up.growth.is_empty());
        // Then it is listed without a new event, and the growth is all there.
        let up = tr.apply(&[], i64::MAX, 2 * period);
        assert_eq!((up.listed, up.deferred), (1, 0));
        assert!(growth(&up, &dir) >= 3 * MB as i64, "{up:?}");
    }

    #[test]
    fn steady_folders_wait_longer_and_growing_ones_dont() {
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        let (cache, logs) = (root.join("cache"), root.join("logs"));
        fs::create_dir(&cache).unwrap();
        fs::create_dir(&logs).unwrap();
        let period = 1800;
        let mut tr = Tracker::new(Rules::default().depth(&root, 1), None, period);
        let both = [event(&cache), event(&logs)];
        let mut listed = Vec::new();
        for p in 0..12 {
            // The logs grow faster than steady; the cache churns a little.
            write(&logs.join(p.to_string()), STEADY as usize + MB as usize);
            write(&cache.join("entry"), (p % 2 + 1) as usize * MB as usize);
            let up = tr.apply(&both, i64::MAX, p * period);
            listed.push(up.listed);
            if p > 0 {
                assert!(growth(&up, &logs) > STEADY as i64, "period {p}: {up:?}");
            }
        }
        // The logs every period; the cache after 1, 2, then 4 periods.
        assert_eq!(listed, [2, 2, 1, 2, 1, 1, 1, 2, 1, 1, 1, 2]);
        // What the cache changed while it waited still counts.
        write(&cache.join("more"), 3 * MB as usize);
        // (The logs, which grew last time, are listed again regardless.)
        let up = tr.apply(&[event(&cache)], i64::MAX, 12 * period);
        assert_eq!((up.listed, up.deferred), (1, 1));
        let up = tr.apply(&[], i64::MAX, 15 * period);
        assert!(growth(&up, &cache) >= 3 * MB as i64, "{up:?}");
    }

    #[test]
    fn a_period_lists_its_budget_most_overdue_first() {
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        let (a, b) = (root.join("a"), root.join("b"));
        for dir in [&a, &b] {
            fs::create_dir(dir).unwrap();
            for i in 0..DEFER_UNIT {
                fs::write(dir.join(i.to_string()), b"").unwrap();
            }
        }
        let period = 1800;
        let mut tr = Tracker::new(Rules::default().depth(&root, 1), None, period);
        tr.budget = DEFER_UNIT as usize;
        // Seen for the first time: listed whatever the budget.
        let up = tr.apply(&[event(&a), event(&b)], i64::MAX, 0);
        assert_eq!((up.listed, up.deferred), (2, 0));
        // Both due, room for one: the other waits.
        write(&b.join("big.bin"), 3 * MB as usize);
        let up = tr.apply(&[event(&a), event(&b)], i64::MAX, period);
        assert_eq!((up.listed, up.deferred), (1, 1));
        assert_eq!(growth(&up, &b), 0);
        // Next period it is the most overdue, and goes first.
        let up = tr.apply(&[event(&a)], i64::MAX, 2 * period);
        assert_eq!((up.listed, up.deferred), (1, 1));
        assert!(growth(&up, &b) >= 3 * MB as i64, "{up:?}");
    }

    #[test]
    fn regrowth_after_a_cleanup_is_measured() {
        let gb = 1_000_000_000i64;
        let series = HashMap::from([
            (
                "/cache".to_string(),
                vec![(0, gb), (10, -6 * gb), (20, 2 * gb), (30, 2 * gb)],
            ),
            ("/steady".to_string(), vec![(0, gb), (10, gb)]),
        ]);
        let r = regrowth(&series, gb as u64);
        assert_eq!(r.len(), 1);
        assert_eq!(
            (r[0].at, r[0].freed, r[0].back),
            (10, 6 * gb as u64, 4 * gb)
        );
        assert!((r[0].share() - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn whole_volume_rescans_are_gaps() {
        let mut tr = Tracker::new(Rules::default(), None, 1);
        let e = Event {
            path: PathBuf::from("/"),
            flags: flag::MUST_SCAN_SUBDIRS,
            id: 1,
        };
        let up = tr.apply(&[e], 0, 1);
        assert_eq!((up.gaps, up.listed), (1, 0));
    }
}
