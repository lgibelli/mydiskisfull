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
use std::ffi::{OsStr, OsString};
use std::io;
use std::ops::Bound;
use std::path::{Path, PathBuf};

use crate::os::bulk::{self, Detail, Entry, Kind};
use crate::os::changes::{Event, flag};
use crate::os::fs::FsId;

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

/// What a .NET build makes beside its project file (`App.csproj`): build
/// output only with one there ([`clean::build_output`]), so not among
/// [`ARTIFACTS`], which the rest takes by name (`bin` is also where people
/// keep their scripts). Owners split there on Windows, where .NET is at
/// home.
///
/// [`clean::build_output`]: crate::clean::build_output
pub const DOTNET_OUTPUT: &[&str] = &["bin", "obj"];

/// Whether a folder called `name` may be .NET's build output ([`DOTNET_OUTPUT`]):
/// on Windows whatever its case (`Bin` is the folder a build writes `bin` to
/// there), and never on other systems, where nothing suggests it.
pub fn dotnet_output(name: &str) -> bool {
    cfg!(windows) && DOTNET_OUTPUT.iter().any(|d| d.eq_ignore_ascii_case(name))
}

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

/// Whether `name`, a folder in the home folder, is a code folder
/// ([`CODE_ROOTS`]), whatever its case: where the disk ignores case,
/// `~/Work` is `~/work`.
pub fn is_code_root(name: &str) -> bool {
    CODE_ROOTS.iter().any(|r| r.eq_ignore_ascii_case(name))
}

/// The folders projects are in: what the home folder holds under a name
/// [`CODE_ROOTS`] has, whatever the case, as the disk spells it (where it
/// ignores case, "Code" and "code" are one folder, to be looked at once),
/// and on Windows Visual Studio's `source\repos`. Folders, or links (the
/// caller says whether it follows one), that may be opened: a link, where
/// it leads too (`~/devel` linked to a folder in Documents).
pub fn code_folders(home: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() || t.is_symlink()))
        .filter(|e| e.file_name().to_str().is_some_and(is_code_root))
        .map(|e| e.path())
        .collect();
    if cfg!(windows) {
        let repos = home.join("source").join("repos");
        if std::fs::symlink_metadata(&repos).is_ok() {
            out.push(repos);
        }
    }
    out.retain(|r| {
        privacy::may_open(r)
            && std::fs::read_link(r)
                .map(|to| crate::keep::resolved(&r.parent().unwrap_or(home).join(to)))
                .map_or(true, |to| privacy::may_open(&to))
    });
    out.sort();
    out
}

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

/// Known folders asked where they are now, at most, when one that was tracked is
/// gone from where it was.
const MAX_LOOKUPS: usize = 8;

/// Folders being followed ([`Tracker::following`]) kept across a restart at
/// most: each is listed in the first period after it, whatever it costs.
pub const MAX_FOLLOWED: usize = 256;

/// A file directly in a folder above where owners are ([`Rules::above_owners`])
/// is an owner of its own from this size: smaller ones aren't anyone's.
const LOOSE_MIN: u64 = 1 << 20;
/// Of those, the biggest followed per folder at most.
const MAX_LOOSE: usize = 1024;

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
    /// The rules for this computer, with `home` as the user's home folder.
    pub fn for_home(home: &Path) -> Rules {
        if cfg!(windows) {
            Rules::for_windows(home)
        } else {
            Rules::for_mac(home)
        }
    }

    /// The rules for Windows: Program Files and the profile's folders, one name
    /// deep, so what a program keeps is told from what another does. `home` is
    /// the profile folder, `C:\Users\me`; the system's own folders are on the
    /// drive Windows says it is on (the profile's if that can't be told). Names
    /// are as Windows spells them (the journal and the listing give them so).
    pub fn for_windows(home: &Path) -> Rules {
        #[cfg(windows)]
        let system = crate::os::sys::system_directory();
        #[cfg(not(windows))]
        let system = home.to_path_buf();
        Rules::for_windows_on(home, &system)
    }

    /// [`Rules::for_windows`] with the system folder given: `C:\Windows` where
    /// Windows is, which needn't be the drive the profile is on.
    pub fn for_windows_on(home: &Path, system_dir: &Path) -> Rules {
        let root_of = |p: &Path| -> PathBuf {
            p.components()
                .take_while(|c| {
                    matches!(
                        c,
                        std::path::Component::Prefix(_) | std::path::Component::RootDir
                    )
                })
                .collect()
        };
        let drive = root_of(home);
        let system = root_of(system_dir);
        // `a/b` as its names, so the path has no mixed separators.
        let below =
            |base: &Path, p: &str| p.split('/').fold(base.to_path_buf(), |acc, n| acc.join(n));
        let at = |p: &str| below(&system, p);
        let mut r = Rules { rules: Vec::new() }
            .depth(drive.clone(), 1)
            .depth(system.clone(), 1)
            .depth(at("Users"), 1)
            .depth(at("Program Files"), 1)
            .depth(at("Program Files (x86)"), 1)
            .depth(at("ProgramData"), 1)
            .depth(at("Windows"), 1)
            .depth(at("Windows/Installer"), 1)
            .depth(at("Windows/SoftwareDistribution"), 1)
            // The recycle bin of every user is one owner.
            .depth(at("$Recycle.Bin"), 0)
            .depth(home, 1)
            .depth(home.join(".cache"), 1);
        // The other profiles of a `Users` folder that isn't on the system drive.
        if let Some(users) = home.parent().filter(|u| {
            root_of(u) != *u
                && u.file_name()
                    .is_some_and(|n| n.eq_ignore_ascii_case("Users"))
        }) {
            r = r.depth(users, 1);
        }
        for sub in [
            "AppData",
            "AppData/Local",
            "AppData/LocalLow",
            "AppData/Roaming",
            "AppData/Local/Packages",
            "AppData/Local/Temp",
            "AppData/Local/Programs",
            "AppData/Local/Microsoft",
            "AppData/Local/Google",
        ] {
            r = r.depth(below(home, sub), 1);
        }
        for media in [
            "Documents",
            "Desktop",
            "Pictures",
            "Videos",
            "Music",
            "OneDrive",
        ] {
            r = r.depth(home.join(media), 1);
        }
        for code in CODE_ROOTS {
            r = r.projects(home.join(code));
        }
        // Where Visual Studio puts them.
        r.projects(home.join("source").join("repos"))
    }

    /// The rules for a Mac with `home` as the user's home folder.
    fn for_mac(home: &Path) -> Rules {
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

    /// Whether `owner` is a prefix whose folders own themselves: then it
    /// owns only the files directly in it (`~/Movies`, the home folder), and
    /// its growth is theirs alone.
    pub fn owns_only_its_files(&self, owner: &Path) -> bool {
        self.rules
            .iter()
            .any(|(p, d)| p == owner && *d != Depth::Fixed(0))
    }

    pub fn owner_of(&self, path: &Path) -> PathBuf {
        let (mut owner, rest, take, _) = self.place(path);
        owner.extend(&rest[..take]);
        owner
    }

    /// Whether `dir` is above where owners are: a rule's prefix (the home
    /// folder, a drive's root) or a level between it and its owners. No folder
    /// owns the files directly in it, so each is an owner of its own: a log
    /// growing in the home folder isn't the whole home folder growing.
    pub fn above_owners(&self, dir: &Path) -> bool {
        self.place(dir).3
    }

    /// The rule `path` is under: its prefix, the names below it, how many of
    /// those name its owner, and whether it's above where owners are.
    fn place<'a>(&self, path: &'a Path) -> (PathBuf, Vec<&'a OsStr>, usize, bool) {
        let (prefix, depth) = match self
            .rules
            .iter()
            .filter(|(p, _)| path.starts_with(p))
            .max_by_key(|(p, _)| p.components().count())
        {
            Some((p, d)) => (p.clone(), d.clone()),
            // No rule covers it (another drive): one name below its root.
            None => (
                path.components()
                    .take_while(|c| {
                        matches!(
                            c,
                            std::path::Component::Prefix(_) | std::path::Component::RootDir
                        )
                    })
                    .collect(),
                Depth::Fixed(1),
            ),
        };
        let rest: Vec<&OsStr> = path
            .strip_prefix(&prefix)
            .map(|r| r.iter().collect())
            .unwrap_or_default();
        let (take, above) = match depth {
            Depth::Fixed(n) => (n.min(rest.len()), rest.len() < n),
            Depth::Projects => (project_depth(&rest), rest.is_empty()),
        };
        (prefix, rest, take, above)
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
    let artifact = |c: &str| ARTIFACTS.contains(&c) || dotnet_output(c);
    rest[1..]
        .iter()
        .position(|c| c.to_str().is_some_and(artifact))
        .map_or(1, |i| i + 2)
}

/// Bytes a directory entry adds to its folder: allocated (stable when a
/// snapshot is taken or released, unlike private size), split among hard
/// links and pure clones. Cloud placeholders and folders add nothing.
pub fn entry_size(e: &Entry) -> u64 {
    if e.kind == Kind::Dir || e.is_dataless() {
        return 0;
    }
    // A count that couldn't be had (Windows) is taken as two names.
    let names = if e.nlink == crate::os::bulk::LINKS_UNKNOWN {
        2
    } else {
        e.nlink
    };
    let mut share = names.max(1) as u64;
    if e.may_share_blocks() {
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
    /// its journal was reset), which are skipped, and rescans cut short by
    /// the period's [`MAX_RESCAN`]: what was measured is less than what
    /// happened.
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
    /// Changed but deferred, or listed again next period whatever the
    /// journal says.
    pending: BTreeSet<PathBuf>,
    /// Of those, the ones listed again because they grow or had a file
    /// written (one held open is silent in the journal until it's closed):
    /// what a restart must not forget ([`Tracker::following`]).
    followed: BTreeSet<PathBuf>,
    /// The files directly in folders above where owners are, each an owner
    /// ([`Rules::above_owners`]): their sizes as last listed.
    loose: HashMap<PathBuf, BTreeMap<OsString, u64>>,
    /// Of those, the folders with more such files than are followed
    /// ([`MAX_LOOSE`]) when last listed: the smallest followed then.
    loose_cut: HashMap<PathBuf, u64>,
    /// Seconds per period, the unit of deferral.
    period: i64,
    /// Entries a period lists before folders seen before wait.
    budget: usize,
    /// Folders a period's rescans list at most ([`MAX_RESCAN`]).
    rescan: usize,
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
            followed: BTreeSet::new(),
            loose: HashMap::new(),
            loose_cut: HashMap::new(),
            period,
            budget: PERIOD_ENTRIES,
            rescan: MAX_RESCAN,
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

    /// The folders listed again whatever the journal says (growing, or with
    /// a file written: an open file is silent in NTFS's journal until it's
    /// closed), at most [`MAX_FOLLOWED`]: kept with the journal's checkpoint,
    /// so a restart goes on looking at them ([`Tracker::resume`]).
    pub fn following(&self) -> Vec<PathBuf> {
        self.followed.iter().take(MAX_FOLLOWED).cloned().collect()
    }

    /// Goes on looking at the folders a tracker before a restart was
    /// following: each is listed next period (as a folder seen for the first
    /// time), and followed again if it grows.
    pub fn resume(&mut self, dirs: impl IntoIterator<Item = PathBuf>) {
        for dir in dirs.into_iter().take(MAX_FOLLOWED) {
            self.pending.insert(dir.clone());
            self.followed.insert(dir);
        }
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
                // At the root or a name below it: too much to rescan, a gap.
                let names = dir
                    .components()
                    .filter(|c| matches!(c, std::path::Component::Normal(_)))
                    .count();
                if names <= 1 {
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
        let mut budget = self.rescan;
        // Folders listed before that are due, and how overdue (1: just due).
        let mut due: Vec<(f64, PathBuf)> = Vec::new();
        // Folders seen for the first time, listed last: one that moved here is
        // found gone from its old place by listing that place, and what was
        // known of it moves along, so it's then measured against what it held
        // rather than counted as freed there and new here. Rescans, and new
        // folders holding known ones (an old place, seen first itself), go
        // first.
        let mut first: Vec<PathBuf> = Vec::new();
        for (dir, recursive) in dirty {
            let (wait, seen) = match self.dirs.get(&dir) {
                Some(d) if !recursive => (d.defer() as i64 * self.period, d.seen),
                _ if !recursive && self.known_children(&dir).is_empty() => {
                    first.push(dir);
                    continue;
                }
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
        for dir in first {
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
        // Folders below left out of a rescan: its budget ran out.
        let mut cut = false;
        while let Some(dir) = stack.pop() {
            if !privacy::may_open(&dir) {
                up.skipped += 1;
                self.followed.remove(&dir);
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
                    let before = self.dirs.get(&dir).copied();
                    // Made since it was last listed (it may have waited
                    // periods), or, seen for the first time, since the last
                    // period.
                    let since = before.map_or(created_after, |b| b.seen);
                    let (mut own, mut created) = (0u64, 0u64);
                    let mut written = false;
                    let above = self.rules.above_owners(&dir);
                    // Each an owner (name, size, made since).
                    let mut loose: Vec<(&OsStr, u64, bool)> = Vec::new();
                    for e in &entries {
                        if e.kind == Kind::Dir {
                            // Never into another volume mounted here.
                            if recursive && !e.is_dataless() && !e.mount_point {
                                if *budget > 0 {
                                    *budget -= 1;
                                    stack.push(dir.join(&e.name));
                                } else {
                                    cut = true;
                                }
                            }
                            continue;
                        }
                        let size = entry_size(e);
                        own += size;
                        if e.crtime > created_after {
                            created += size;
                        }
                        written |= e.mtime > created_after;
                        if above && size >= LOOSE_MIN {
                            loose.push((e.name.as_os_str(), size, e.crtime > since));
                        }
                    }
                    let meta = std::fs::symlink_metadata(&dir).ok();
                    // Made since the last period: all it holds came since, a
                    // file moved in too (it keeps its own creation time).
                    let made = meta
                        .as_ref()
                        .and_then(|m| m.created().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .is_some_and(|d| d.as_secs() as i64 > created_after);
                    let mut state = Dir {
                        own,
                        entries: entries.len().min(u32::MAX as usize) as u32,
                        seen: now,
                        steady: 0,
                        inode: meta.map_or(0, |m| crate::portable::file_id(&dir, &m)),
                    };
                    if let Some(b) = before {
                        let periods = ((now - b.seen + self.period / 2) / self.period).max(1);
                        if own.abs_diff(b.own) < self.steady * periods as u64 {
                            state.steady = (b.steady + 1).min(MAX_STEADY);
                        }
                    }
                    self.dirs.insert(dir.clone(), state);
                    let delta = match before {
                        Some(b) => own as i64 - b.own as i64,
                        None if made => own as i64,
                        None => created as i64,
                    };
                    if above {
                        self.credit_loose(&dir, before.is_some(), loose, deltas);
                    } else if delta != 0 {
                        *deltas.entry(self.rules.owner_of(&dir)).or_default() += delta;
                    }
                    // Growing: listed again next period, whatever FSEvents says.
                    let growing = delta > 0 && delta >= self.steady as i64;
                    // NTFS writes a journal record for a file when it is first
                    // changed and again when it is closed, so a file kept open
                    // while it grows is silent in between: a folder with a file
                    // written since the last period, or seen for the first time,
                    // is looked at again.
                    let silent = cfg!(windows) && (written || before.is_none());
                    // One followed before a restart: seen first again, and
                    // looked at again for what it grows from now on.
                    let resumed = before.is_none() && self.followed.contains(&dir);
                    if growing || silent || resumed {
                        self.pending.insert(dir.clone());
                        self.followed.insert(dir.clone());
                    } else {
                        self.followed.remove(&dir);
                    }
                }
                Err(e) if is_gone(&e) => {
                    self.followed.remove(&dir);
                    self.gone(&dir, deltas);
                }
                Err(_) => {
                    self.followed.remove(&dir);
                    up.unreadable += 1;
                }
            }
        }
        // What changed below the folders left out isn't known: a gap, not a
        // period that saw everything.
        if cut {
            up.gaps += 1;
        }
    }

    /// Credits each file directly in `dir`, a folder above where owners are, to
    /// itself: what it grew since `dir` was last listed (`known`), or, the
    /// first time, its size if it's new since the last period. A file no
    /// longer there (or now under [`LOOSE_MIN`]) is freed. Each file is
    /// `(name, size, made since dir was last listed)`.
    fn credit_loose(
        &mut self,
        dir: &Path,
        known: bool,
        mut files: Vec<(&OsStr, u64, bool)>,
        deltas: &mut HashMap<PathBuf, i64>,
    ) {
        // The biggest are followed. One there but not among them when last
        // listed was at most the smallest followed then, unless it's made
        // since: one that comes in grew at least past that. One that drops
        // out changed by what it's listed at now, and isn't followed after.
        let before = self.loose.remove(dir).filter(|_| known);
        let was_cut = self.loose_cut.remove(dir).filter(|_| before.is_some());
        // Of the files followed before, what each is at now: one may drop out.
        let mut listed: HashMap<&OsStr, u64> = HashMap::new();
        let mut cutoff = None;
        if files.len() > MAX_LOOSE {
            if let Some(b) = &before {
                listed = files
                    .iter()
                    .filter(|f| b.contains_key(f.0))
                    .map(|f| (f.0, f.1))
                    .collect();
            }
            files.sort_by_key(|f| Reverse(f.1));
            files.truncate(MAX_LOOSE);
            cutoff = files.last().map(|f| f.1);
        }
        let mut now = BTreeMap::new();
        for (name, size, new) in files {
            let was = match &before {
                Some(b) => match (b.get(name), was_cut) {
                    (Some(&was), _) => was,
                    (None, Some(smallest)) if !new => size.min(smallest),
                    (None, _) => 0,
                },
                None if new => 0,
                None => size,
            };
            if size != was {
                *deltas.entry(dir.join(name)).or_default() += size as i64 - was as i64;
            }
            now.insert(name.to_os_string(), size);
        }
        for (name, was) in before.into_iter().flatten() {
            if !now.contains_key(&name) {
                // Gone, or under LOOSE_MIN, if it isn't listed below them.
                let size = listed.get(name.as_os_str()).copied().unwrap_or(0);
                if size != was {
                    *deltas.entry(dir.join(&name)).or_default() += size as i64 - was as i64;
                }
            }
        }
        if let Some(smallest) = cutoff {
            self.loose_cut.insert(dir.to_path_buf(), smallest);
        }
        self.loose.insert(dir.to_path_buf(), now);
    }

    /// The children of `dir` that it, or folders known below them, are
    /// known by.
    fn known_children(&self, dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut next = self
            .dirs
            .range::<PathBuf, _>((Bound::Excluded(dir.to_path_buf()), Bound::Unbounded))
            .next()
            .map(|(p, _)| p);
        while let Some(p) = next {
            let Some(name) = p.strip_prefix(dir).ok().and_then(|r| r.iter().next()) else {
                break;
            };
            let child = dir.join(name);
            // Past all that's below it: paths compare by component, and no
            // name sorts after 0xFF where names are UTF-8 (APFS)...
            let past = child.join(crate::portable::name_after_all());
            // ...and where they aren't, step over what still is below it.
            next = self
                .dirs
                .range::<PathBuf, _>((Bound::Excluded(past), Bound::Unbounded))
                .map(|(q, _)| q)
                .find(|q| !q.starts_with(&child));
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
            let followed = self.followed.remove(&p);
            // Its files each an owner, or all its folder's.
            let loose = self.loose.remove(&p);
            let cutoff = self.loose_cut.remove(&p);
            match &loose {
                Some(files) => {
                    for (name, size) in files {
                        *deltas.entry(p.join(name)).or_default() -= *size as i64;
                    }
                }
                None => *deltas.entry(self.rules.owner_of(&p)).or_default() -= d.own as i64,
            }
            let Some(to) = &to else { continue };
            let rel = p.strip_prefix(root).expect("below it");
            let moved = if rel.as_os_str().is_empty() {
                to.clone()
            } else {
                to.join(rel)
            };
            if !self.dirs.contains_key(&moved) {
                match loose.filter(|_| self.rules.above_owners(&moved)) {
                    Some(files) => {
                        for (name, size) in &files {
                            *deltas.entry(moved.join(name)).or_default() += *size as i64;
                        }
                        if let Some(smallest) = cutoff {
                            self.loose_cut.insert(moved.clone(), smallest);
                        }
                        self.loose.insert(moved.clone(), files);
                    }
                    None => *deltas.entry(self.rules.owner_of(&moved)).or_default() += d.own as i64,
                }
                if pending {
                    self.pending.insert(moved.clone());
                }
                if followed {
                    self.followed.insert(moved.clone());
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
        // A few tried, not all: after a big tree is deleted none resolves, and a
        // lookup of each of thousands would cost more than the period.
        below.iter().take(MAX_LOOKUPS).find_map(|p| {
            let d = self.dirs.get(p).filter(|d| d.inode != 0)?;
            let now = crate::os::fs::path_of(fsid, d.inode).ok()?;
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
        self.followed.retain(|p| pending.contains(p));
        let dirs = &self.dirs;
        self.loose.retain(|p, _| dirs.contains_key(p));
        let loose = &self.loose;
        self.loose_cut.retain(|p, _| loose.contains_key(p));
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
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const MB: u64 = 1_000_000;

    #[cfg(not(windows))]
    fn rules() -> Rules {
        Rules::for_home(Path::new("/Users/me"))
    }

    #[cfg(not(windows))]
    fn owner(p: &str) -> String {
        rules().owner_of(Path::new(p)).display().to_string()
    }

    #[cfg(windows)]
    fn owner(p: &str) -> String {
        Rules::for_windows_on(Path::new(r"C:\Users\me"), Path::new(r"C:\Windows\System32"))
            .owner_of(Path::new(p))
            .display()
            .to_string()
    }

    #[cfg(windows)]
    #[test]
    fn a_profile_on_another_drive_keeps_its_own_owners_and_the_systems() {
        let r = Rules::for_windows_on(Path::new(r"D:\Users\me"), Path::new(r"C:\Windows\System32"));
        let o = |p: &str| r.owner_of(Path::new(p)).display().to_string();
        assert_eq!(
            o(r"C:\Program Files\Docker\bin"),
            r"C:\Program Files\Docker"
        );
        assert_eq!(o(r"C:\Windows\WinSxS\x"), r"C:\Windows\WinSxS");
        assert_eq!(
            o(r"D:\Users\me\AppData\Local\Slack\x"),
            r"D:\Users\me\AppData\Local\Slack"
        );
        // Another profile there is one owner, not all of them together.
        assert_eq!(o(r"D:\Users\other\AppData\x"), r"D:\Users\other");
        assert_eq!(o(r"D:\Games\x.pak"), r"D:\Games");
    }

    #[cfg(windows)]
    #[test]
    fn owners_are_folders_a_person_recognises_on_windows() {
        let same = |p: &str, o: &str| assert_eq!(owner(p), o, "{p}");
        same(
            r"C:\Users\me\AppData\Local\Spotify\Data\x",
            r"C:\Users\me\AppData\Local\Spotify",
        );
        same(
            r"C:\Users\me\AppData\Roaming\Slack\Cache\0a",
            r"C:\Users\me\AppData\Roaming\Slack",
        );
        same(
            r"C:\Users\me\AppData\Local\Packages\CanonicalGroup.Ubuntu\LocalState\ext4.vhdx",
            r"C:\Users\me\AppData\Local\Packages\CanonicalGroup.Ubuntu",
        );
        same(r"C:\Users\me\Downloads\big.iso", r"C:\Users\me\Downloads");
        same(r"C:\Users\me\.cargo\registry\cache", r"C:\Users\me\.cargo");
        same(r"C:\Users\me", r"C:\Users\me");
        same(r"C:\Users\other\x", r"C:\Users\other");
        same(
            r"C:\Program Files\Docker\Docker\bin",
            r"C:\Program Files\Docker",
        );
        same(
            r"C:\ProgramData\Package Cache\x",
            r"C:\ProgramData\Package Cache",
        );
        same(r"C:\Windows\WinSxS\amd64_x", r"C:\Windows\WinSxS");
        same(r"C:\$Recycle.Bin\S-1-5-21\$R123", r"C:\$Recycle.Bin");
        same(r"C:\hiberfil.sys", r"C:\hiberfil.sys");
        // Another drive has no rule of its own: one name below its root.
        same(r"D:\Games\Big\x.pak", r"D:\Games");
        // Projects, as on the Mac.
        same(
            r"C:\Users\me\devel\app\target\debug\deps",
            r"C:\Users\me\devel\app\target",
        );
        same(
            r"C:\Users\me\devel\app\src\main.rs",
            r"C:\Users\me\devel\app",
        );
        // Visual Studio's projects, and what a .NET build makes beside one.
        same(
            r"C:\Users\me\source\repos\Shop\Shop.Web\bin\Debug\net8.0\Shop.Web.dll",
            r"C:\Users\me\source\repos\Shop\Shop.Web\bin",
        );
        same(
            r"C:\Users\me\source\repos\Shop\Shop.Web\Pages\Index.cshtml",
            r"C:\Users\me\source\repos\Shop",
        );
        same(
            r"C:\Users\me\devel\api\obj\project.assets.json",
            r"C:\Users\me\devel\api\obj",
        );
        same(r"C:\Users\me\source\notes.txt", r"C:\Users\me\source");
        assert!(
            !Rules::for_windows(Path::new(r"C:\Users\me"))
                .owner_of(Path::new(r"C:\Users\me\AppData\Local\x"))
                .to_string_lossy()
                .contains('/'),
            "no mixed separators"
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_folders_above_owners_are_the_top_ones_on_windows() {
        let r = Rules::for_windows_on(Path::new(r"C:\Users\me"), Path::new(r"C:\Windows\System32"));
        let above = |p: &str| r.above_owners(Path::new(p));
        for top in [r"C:\", r"C:\Users\me", r"C:\Users\me\AppData\Local", r"D:\"] {
            assert!(above(top), "{top}");
        }
        for owned in [
            r"C:\$Recycle.Bin",
            r"C:\Users\me\Downloads",
            r"C:\Windows\WinSxS",
            r"D:\Games",
        ] {
            assert!(!above(owned), "{owned}");
        }
        assert_eq!(owner(r"C:\pagefile.sys"), r"C:\pagefile.sys");
    }

    #[cfg(unix)]
    #[test]
    fn a_name_past_the_bound_is_still_below_its_folder() {
        use std::os::unix::ffi::OsStrExt;
        let mut tr = Tracker::new(Rules::default(), None, 1);
        let dir = Path::new("/a");
        let dir_state = Dir {
            own: 0,
            entries: 0,
            seen: 0,
            steady: 0,
            inode: 0,
        };
        // "\xffa" sorts after the "\xff" the skip jumps to.
        let odd = dir.join("b").join(std::ffi::OsStr::from_bytes(b"\xffa"));
        for p in [dir.join("b"), odd, dir.join("c")] {
            tr.dirs.insert(p, dir_state);
        }
        assert_eq!(tr.known_children(dir), [dir.join("b"), dir.join("c")]);
    }

    #[cfg(not(windows))]
    #[test]
    fn a_prefix_owns_only_the_files_directly_in_it() {
        let r = rules();
        for p in ["/Users/me", "/Users/me/Movies", "/Users/me/devel"] {
            assert!(r.owns_only_its_files(Path::new(p)), "{p}");
            // The files listed in it are its own; a folder in it owns itself.
            assert_eq!(owner(p), p);
            assert_ne!(owner(&format!("{p}/Project/x")), p);
        }
        // An owner below a prefix owns all of it; the Trash, all of it too.
        for p in ["/Users/me/Movies/Project", "/Users/me/.Trash"] {
            assert!(!r.owns_only_its_files(Path::new(p)), "{p}");
        }
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

    #[cfg(not(windows))]
    #[test]
    fn the_folders_above_owners_are_the_top_ones() {
        let r = Rules::for_home(Path::new("/Users/me"));
        let above = |p: &str| r.above_owners(Path::new(p));
        for top in [
            "/",
            "/Users/me",
            "/Users/me/Library",
            "/Users/me/Documents",
            "/Users/me/devel",
            // Owners are two levels down: one in between.
            "/Users/me/Library/Developer/Xcode",
        ] {
            assert!(above(top), "{top}");
        }
        for owned in [
            "/Users/me/Documents/Taxes",
            "/Users/me/Downloads",
            "/Users/me/devel/app",
            "/Users/me/Library/Caches/com.spotify.client",
            // All of it one owner.
            "/Users/me/.Trash",
        ] {
            assert!(!above(owned), "{owned}");
        }
    }

    #[cfg(not(windows))]
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
        let mut f = fs::File::create(p).unwrap();
        // Not one repeated byte: a compressed folder would take less than it holds.
        let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ len as u64;
        let data: Vec<u8> = (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect();
        std::io::Write::write_all(&mut f, &data).unwrap();
        f.sync_all().unwrap();
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
    fn a_file_into_or_out_of_the_biggest_followed_is_credited_what_is_known() {
        let mut tr = Tracker::new(Rules::default(), None, 1);
        let dir = Path::new("/above");
        let names: Vec<OsString> = (0..=MAX_LOOSE + 1)
            .map(|i| OsString::from(format!("f{i}")))
            .collect();
        // `n` files, f0 the smallest, `new` the ones made since.
        let listing = |sizes: &[u64], new: &[usize]| -> Vec<(&OsStr, u64, bool)> {
            names
                .iter()
                .zip(sizes)
                .enumerate()
                .filter(|(_, (_, s))| **s > 0)
                .map(|(i, (n, s))| (n.as_os_str(), *s, new.contains(&i)))
                .collect()
        };
        let at =
            |deltas: &HashMap<PathBuf, i64>, i: usize| deltas.get(&dir.join(&names[i])).copied();
        let mut sizes: Vec<u64> = (0..=MAX_LOOSE as u64).map(|i| (2 + i) * MB).collect();
        let mut deltas = HashMap::new();
        tr.credit_loose(dir, false, listing(&sizes, &[]), &mut deltas);
        assert!(deltas.is_empty(), "{deltas:?}");
        // f0, not followed, becomes the biggest: it was at most the smallest
        // followed (f1, 3 MB), so it grew at least past that. f1 isn't
        // followed any more, and didn't change.
        sizes[0] = 5_000 * MB;
        tr.credit_loose(dir, true, listing(&sizes, &[]), &mut deltas);
        assert_eq!(at(&deltas, 0), Some(4_997 * MB as i64));
        assert_eq!(deltas.len(), 1, "{deltas:?}");
        // A file made since is new; one deleted is freed.
        deltas.clear();
        sizes.push(3_000 * MB);
        let deleted = sizes[MAX_LOOSE];
        sizes[MAX_LOOSE] = 0;
        tr.credit_loose(dir, true, listing(&sizes, &[MAX_LOOSE + 1]), &mut deltas);
        assert_eq!(at(&deltas, MAX_LOOSE + 1), Some(3_000 * MB as i64));
        assert_eq!(at(&deltas, MAX_LOOSE), Some(-(deleted as i64)));
        assert_eq!(deltas.len(), 2, "{deltas:?}");
        // One that shrinks out of the biggest freed what it shrank by; f1,
        // which comes in again as it was, didn't grow.
        deltas.clear();
        let was = sizes[MAX_LOOSE - 1];
        sizes[MAX_LOOSE - 1] = 3 * MB / 2;
        tr.credit_loose(dir, true, listing(&sizes, &[]), &mut deltas);
        assert_eq!(
            at(&deltas, MAX_LOOSE - 1),
            Some(sizes[MAX_LOOSE - 1] as i64 - was as i64)
        );
        assert_eq!(deltas.len(), 1, "{deltas:?}");

        // Exactly as many as are followed is all of them: one that grows past
        // the smallest that count, and comes in, grew.
        let mut tr = Tracker::new(Rules::default(), None, 1);
        let mut deltas = HashMap::new();
        let mut sizes: Vec<u64> = (0..MAX_LOOSE as u64).map(|i| (2 + i) * MB).collect();
        tr.credit_loose(dir, false, listing(&sizes, &[]), &mut deltas);
        sizes.push(4_000 * MB);
        tr.credit_loose(dir, true, listing(&sizes, &[]), &mut deltas);
        assert_eq!(
            at(&deltas, MAX_LOOSE),
            Some(4_000 * MB as i64),
            "{deltas:?}"
        );
        assert_eq!(deltas.len(), 1, "{deltas:?}");
    }

    #[test]
    fn a_restart_goes_on_following_a_growing_folder() {
        // A download kept open: the journal says nothing more of it until
        // it's closed, so a watcher that restarts must remember its folder.
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        let film = root.join("Storage/film");
        let file = film.join("film.mkv");
        write(&file, 20 * MB as usize);
        let rules = || Rules::default().depth(&root, 1);
        let mut tr = Tracker::new(rules(), None, 1);
        tr.apply(&[event(&film)], 0, 100);
        assert_eq!(tr.following(), std::slice::from_ref(&film));

        let kept = tr.following();
        let mut tr = Tracker::new(rules(), None, 1);
        tr.resume(kept);
        let append = |mb: u64| {
            let mut f = fs::OpenOptions::new().append(true).open(&file).unwrap();
            std::io::Write::write_all(&mut f, &vec![3u8; (mb * MB) as usize]).unwrap();
            f.sync_all().unwrap();
        };
        // Seen first again (what it grew meanwhile isn't known)...
        append(10);
        let up = tr.apply(&[], i64::MAX, 200);
        assert_eq!(up.listed, 1, "{up:?}");
        // ...and exact from then on, with no event.
        append(30);
        let up = tr.apply(&[], i64::MAX, 300);
        // (Allocated bytes: the last block was partly in use already.)
        assert!(
            growth(&up, &root.join("Storage")) >= 29 * MB as i64,
            "{up:?}"
        );
        // Without what was kept, nothing would have listed it.
        let mut forgetful = Tracker::new(rules(), None, 1);
        assert_eq!(forgetful.apply(&[], i64::MAX, 200).listed, 0);
        // A followed folder that's gone isn't followed any more. (On Windows
        // the folder that held it, listed for the first time, is followed for
        // a period: a file kept open in it would say nothing.)
        fs::remove_dir_all(&film).unwrap();
        tr.apply(&[], i64::MAX, 400);
        tr.apply(&[event(&root.join("Storage"))], i64::MAX, 500);
        let following = tr.following();
        assert!(!following.contains(&film), "{following:?}");
        if cfg!(windows) {
            assert_eq!(following, [root.join("Storage")]);
        } else {
            assert!(following.is_empty(), "{following:?}");
        }
    }

    // Follows a moved folder by its number, which only macOS can do so far.
    #[cfg(target_os = "macos")]
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
        tr.follow_moves(crate::os::fs::fsid(&root).unwrap());
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

    // Follows a moved folder by its number, which only macOS is tested on.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_folder_that_moves_and_grows_in_one_period_takes_its_size_along() {
        let _on = crate::privacy::SWITCH
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        let (incomplete, movies) = (root.join("Downloads/incomplete"), root.join("Movies"));
        let film = incomplete.join("film");
        write(&film.join("film.mkv"), 20 * MB as usize);
        fs::create_dir_all(&movies).unwrap();
        let rules = Rules::default().depth(&root, 1);
        let mut tr = Tracker::new(rules.clone(), None, 1);
        tr.follow_moves(crate::os::fs::fsid(&root).unwrap());
        tr.apply(&[event(&incomplete), event(&film)], 0, 100);

        // Done downloading: moved where films go, and written once more there.
        let moved = movies.join("film");
        fs::rename(&film, &moved).unwrap();
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(moved.join("film.mkv"))
            .unwrap();
        std::io::Write::write_all(&mut f, &vec![3u8; 10 * MB as usize]).unwrap();
        f.sync_all().unwrap();
        let up = tr.apply(
            &[event(&incomplete), event(&movies), event(&moved)],
            i64::MAX,
            200,
        );
        let downloads = growth(&up, &root.join("Downloads"));
        assert!(downloads <= -(20 * MB as i64), "{up:?}");
        // What it held went along, and what it grew there counts too.
        assert!(growth(&up, &movies) >= 29 * MB as i64, "{up:?}");
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
    fn a_new_folder_credits_what_was_moved_into_it() {
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        let (videos, films) = (root.join("Videos"), root.join("Films"));
        let film = videos.join("film.mkv");
        write(&film, 4 * MB as usize);
        let born = fs::metadata(&film)
            .unwrap()
            .created()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let mut tr = Tracker::new(Rules::default().depth(&root, 1), None, 1);
        let up = tr.apply(&[event(&videos)], born, 100);
        assert!(up.growth.is_empty(), "{up:?}");

        // A folder made since, and the film moved into it: the film is older
        // than the last period, the folder isn't.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::create_dir(&films).unwrap();
        fs::rename(&film, films.join("film.mkv")).unwrap();
        let up = tr.apply(&[event(&videos), event(&films)], born, 200);
        assert!(growth(&up, &videos) <= -(4 * MB as i64), "{up:?}");
        assert!(growth(&up, &films) >= 4 * MB as i64, "{up:?}");
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

    // Counts links, which Windows listings don't give yet.
    #[cfg(target_os = "macos")]
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
    fn a_file_directly_in_a_top_folder_is_an_owner_of_its_own() {
        // A log growing in the home folder (or a drive's root) is the log
        // growing, measured against itself, not the whole home folder.
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        let old = root.join("old.bin");
        write(&old, 4 * MB as usize);
        write(&root.join("app/data.bin"), 2 * MB as usize);
        let mut tr = Tracker::new(Rules::default().depth(&root, 1), None, 1);
        // First sight: nothing there is new.
        let up = tr.apply(&[event(&root), event(&root.join("app"))], i64::MAX, 100);
        assert!(up.growth.is_empty(), "{up:?}");

        let log = root.join("app.log");
        write(&log, 3 * MB as usize);
        write(&root.join("small.txt"), 1000);
        let up = tr.apply(&[event(&root)], i64::MAX, 200);
        assert!(growth(&up, &log) >= 3 * MB as i64, "{up:?}");
        assert_eq!(growth(&up, &root), 0, "{up:?}");
        // A small file is nobody's.
        assert_eq!(up.growth.len(), 1, "{up:?}");

        // Deleted: freed under its own name. A folder there is still one.
        fs::remove_file(&old).unwrap();
        fs::remove_dir_all(root.join("app")).unwrap();
        let up = tr.apply(&[event(&root)], i64::MAX, 300);
        assert!(growth(&up, &old) <= -(4 * MB as i64), "{up:?}");
        assert!(growth(&up, &root.join("app")) <= -(2 * MB as i64), "{up:?}");
        assert_eq!(growth(&up, &root), 0, "{up:?}");

        // Seen first after a restart: only a file new since the last period.
        let mut tr = Tracker::new(Rules::default().depth(&root, 1), None, 1);
        let up = tr.apply(&[event(&root)], 0, 100);
        assert!(growth(&up, &log) >= 3 * MB as i64, "{up:?}");
        let mut tr = Tracker::new(Rules::default().depth(&root, 1), None, 1);
        let up = tr.apply(&[event(&root)], i64::MAX, 100);
        assert!(up.growth.is_empty(), "{up:?}");
    }

    #[test]
    fn a_rescan_cut_short_by_its_budget_is_a_gap() {
        let t = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(t.path()).unwrap();
        for name in ["a", "b", "c", "d"] {
            write(&root.join("p").join(name).join("x.bin"), MB as usize);
        }
        let rescan = Event {
            path: root.join("p"),
            flags: flag::MUST_SCAN_SUBDIRS,
            id: 1,
        };
        let mut tr = Tracker::new(Rules::default().depth(&root, 1), None, 1);
        tr.rescan = 2;
        let up = tr.apply(std::slice::from_ref(&rescan), 0, 1);
        // `p` and two of its four folders.
        assert_eq!((up.listed, up.gaps), (3, 1), "{up:?}");
        // Within the budget: no gap.
        let mut tr = Tracker::new(Rules::default().depth(&root, 1), None, 1);
        tr.rescan = 4;
        let up = tr.apply(&[rescan], 0, 1);
        assert_eq!((up.listed, up.gaps), (5, 0), "{up:?}");
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
