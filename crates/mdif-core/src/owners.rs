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
//! period after a start is a lower bound, later ones are exact. A folder
//! moved as a whole (a rename, a move to the Trash) is seen on neither side.
//!
//! Sizes are allocated bytes, which snapshots don't change, divided among
//! hard links and among pure clones so neither is counted twice.
//!
//! Listing costs about 3.5 µs per entry whatever is asked for, so a busy
//! folder with tens of thousands of entries (a browser cache) would eat most
//! of the watcher's budget if it were re-listed every period. Folders are
//! re-listed at most once per [`DEFER_UNIT`] entries' worth of periods (up
//! to [`MAX_DEFER`] periods): their growth still counts in full, a little
//! later.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use mdif_macos::bulk::{self, Detail, EF_MAY_SHARE_BLOCKS, Entry, Kind};
use mdif_macos::fsevents::{Event, flag};

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
/// The longest a changed folder waits to be re-listed, in periods.
pub const MAX_DEFER: u32 = 4;

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
}

impl Dir {
    /// Periods to wait between listings.
    fn defer(&self) -> u32 {
        (self.entries / DEFER_UNIT).clamp(1, MAX_DEFER)
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
    /// Changed directories left for a later period (too big to list often).
    pub deferred: usize,
    pub unreadable: usize,
    /// Whole-volume rescans FSEvents asked for (history lost, e.g. after
    /// its journal was reset), which are skipped.
    pub gaps: usize,
}

/// Directory sizes as last seen, and growth not yet reported.
pub struct Tracker {
    rules: Rules,
    /// Device of the watched volume; directories elsewhere are ignored.
    dev: Option<i32>,
    dirs: BTreeMap<PathBuf, Dir>,
    carry: HashMap<PathBuf, i64>,
    /// Changed but deferred.
    pending: BTreeSet<PathBuf>,
    /// Seconds per period, the unit of deferral.
    period: i64,
}

impl Tracker {
    /// `period` is how often [`Tracker::apply`] is called, in seconds.
    pub fn new(rules: Rules, dev: Option<i32>, period: i64) -> Self {
        Tracker {
            rules,
            dev,
            dirs: BTreeMap::new(),
            carry: HashMap::new(),
            pending: BTreeSet::new(),
            period,
        }
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
        for (dir, recursive) in dirty {
            // Big and listed recently: later. (The slack absorbs periods
            // that run a little early.)
            if !recursive
                && let Some(d) = self.dirs.get(&dir)
                && now - d.seen < d.defer() as i64 * self.period - self.period / 10
            {
                up.deferred += 1;
                self.pending.insert(dir);
                continue;
            }
            self.measure(
                dir,
                recursive,
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
            match bulk::read_dir_with(&dir, Detail::Sizes) {
                Ok(entries) => {
                    // A mount point lists the other volume.
                    if let (Some(dev), Some(first)) = (self.dev, entries.first())
                        && first.dev != dev
                    {
                        continue;
                    }
                    up.listed += 1;
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
                    let state = Dir {
                        own,
                        entries: entries.len().min(u32::MAX as usize) as u32,
                        seen: now,
                    };
                    let delta = match self.dirs.insert(dir.clone(), state) {
                        Some(before) => own as i64 - before.own as i64,
                        None => created as i64,
                    };
                    if delta != 0 {
                        *deltas.entry(self.rules.owner_of(&dir)).or_default() += delta;
                    }
                }
                Err(e) if is_gone(&e) => {
                    // Deleted: it and whatever we knew below it are freed.
                    let gone: Vec<PathBuf> = self
                        .dirs
                        .range(dir.clone()..)
                        .take_while(|(p, _)| p.starts_with(&dir))
                        .map(|(p, _)| p.clone())
                        .collect();
                    for p in gone {
                        let d = self.dirs.remove(&p).expect("just listed");
                        if d.own > 0 {
                            *deltas.entry(self.rules.owner_of(&p)).or_default() -= d.own as i64;
                        }
                    }
                }
                Err(_) => up.unreadable += 1,
            }
        }
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
