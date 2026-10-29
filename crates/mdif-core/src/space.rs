//! Where the space goes: everything on the disk sorted into kinds (macOS,
//! apps, build output, caches, your files ...), biggest first, each with its
//! biggest items, and how much of it is waste: apps not opened in months,
//! build output, caches, files untouched for a year.
//!
//! One scan of the Data volume ([`measure`], as long as the disk map's)
//! makes a [`Measured`]: the folders big enough to name, the container's
//! other volumes, and what macOS frees by itself. [`report`] sorts it into
//! kinds. It's cheap, so a measurement can be kept and shown again later
//! with what you keep now.
//!
//! The same scan lists your big files ([`BigFile`]): those nothing has
//! opened or changed for months are in the report's `old_files`, each with
//! what `mdif clean` would do with it (to the Trash).
//!
//! Every byte is counted once. A folder's bytes, less those of the folders
//! in it that are sorted on their own, belong to the nearest folder above
//! (or itself) that a rule sorts (by its path: `classify`). The kinds add up
//! to what the container uses:
//! - blocks shared by clones, which allocated sizes count once per copy,
//!   are taken off where the clones are, in proportion to their shared
//!   bytes, down to what the scan found really used;
//! - the other volumes (System, Preboot, Recovery, VM) are macOS's;
//! - what the scan can't see (files only macOS can read, file-system
//!   metadata, blocks only snapshots hold) is "System data", in Other,
//!   which says what's in it and offers to delete Time Machine's snapshots.
//!
//! Nothing here reads a file's contents: sizes come from the scan,
//! when apps were last opened from Spotlight's metadata.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::clean::{self, Action, Step};
use crate::keep::Keep;
use crate::map::DATA_ROOT;
use crate::owners::{ARTIFACTS, CODE_ROOTS};
use crate::scan::{self, Node, Options};
use crate::suggest::display;
use crate::units::bytes;

/// What [`Measured`] looks like; a kept one of another version is measured
/// again.
pub const VERSION: u32 = 3;

/// Folders this deep and this big are kept in the tree; smaller ones count
/// in the folder above. Files as big as [`clean::BIG_FILE`] are listed too.
pub const TREE: Options = Options {
    max_depth: 16,
    min_node: 100_000_000,
    big_files: Some(clean::BIG_FILE),
};

/// Your big files kept in a measurement: the biggest this many.
pub const FILES: usize = 3000;

/// The biggest folder [`update`] measures again; above, the breakdown waits
/// for Refresh.
const PATCH_MAX: u64 = 40_000_000_000;

/// Items listed per kind (the app shows the first few, the rest on asking).
pub const ITEMS: usize = 20;

/// An app not opened for this long is unused.
pub const UNUSED: i64 = 90 * DAY;

/// A Rust profile's incremental caches are an item of their own from this
/// size.
pub const INCREMENTAL_MIN: u64 = 500_000_000;

/// An item this much of whose bytes are cold is flagged cold.
const COLD_SHARE: f64 = 0.8;

const DAY: i64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Kind {
    Macos,
    Apps,
    Build,
    Dev,
    Cloud,
    Vms,
    Media,
    Mail,
    Caches,
    Appdata,
    Files,
    Other,
}

impl Kind {
    pub const ALL: [Kind; 12] = [
        Kind::Macos,
        Kind::Apps,
        Kind::Build,
        Kind::Dev,
        Kind::Cloud,
        Kind::Vms,
        Kind::Media,
        Kind::Mail,
        Kind::Caches,
        Kind::Appdata,
        Kind::Files,
        Kind::Other,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Kind::Macos => "macos",
            Kind::Apps => "apps",
            Kind::Build => "build",
            Kind::Dev => "dev",
            Kind::Cloud => "cloud",
            Kind::Vms => "vms",
            Kind::Media => "media",
            Kind::Mail => "mail",
            Kind::Caches => "caches",
            Kind::Appdata => "appdata",
            Kind::Files => "files",
            Kind::Other => "other",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Kind::Macos => "macOS",
            Kind::Apps => "Apps",
            Kind::Build => "Build output",
            Kind::Dev => "Developer tools",
            Kind::Cloud => "Cloud copies",
            Kind::Vms => "Virtual machines",
            Kind::Media => "Photos, music and video",
            Kind::Mail => "Mail and Messages",
            Kind::Caches => "Caches",
            Kind::Appdata => "App data",
            Kind::Files => "Your files",
            Kind::Other => "Other",
        }
    }

    /// Where bytes untouched for a year are worth a look.
    fn cold_counts(self) -> bool {
        matches!(self, Kind::Files | Kind::Media | Kind::Appdata)
    }
}

// ---------------------------------------------------------------------------
// What a scan measured, kept between runs.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measured {
    pub version: u32,
    /// When the scan started (Unix seconds).
    pub measured: i64,
    pub took_secs: f64,
    /// The account's home folder.
    pub home: PathBuf,
    pub volume: VolumeSpace,
    /// The container's volumes besides the Data volume.
    pub volumes: Vec<OtherVolume>,
    /// What APFS says the Data volume uses.
    pub data_in_use: u64,
    /// What the scan found it really uses: every block once.
    pub data_real: u64,
    /// Local Time Machine snapshots of the Data volume.
    pub snapshots: usize,
    /// Folders the scan couldn't read.
    pub unreadable_dirs: u64,
    /// The Data volume, from its root ([`DATA_ROOT`]).
    pub tree: Dir,
    /// When each app was last opened, and added, from Spotlight.
    pub apps: Vec<AppDates>,
    /// Simulator runtimes Xcode knows, by the folder holding each.
    pub runtimes: Vec<Runtime>,
    /// Your own big files ([`clean::yours`]), biggest first, at most
    /// [`FILES`].
    #[serde(default)]
    pub files: Vec<BigFile>,
}

/// A file of yours of at least [`clean::BIG_FILE`] allocated bytes, as the
/// scan found it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BigFile {
    /// As you know it (`/Users/me/...`).
    #[serde(rename = "n")]
    pub path: String,
    #[serde(rename = "a")]
    pub allocated: u64,
    #[serde(rename = "p")]
    pub private: u64,
    /// Last read and last changed (Unix seconds): see [`clean::last_used`].
    #[serde(rename = "r")]
    pub accessed: i64,
    #[serde(rename = "m")]
    pub modified: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeSpace {
    pub total: u64,
    pub used: u64,
    pub free: u64,
    /// What macOS would free by itself for something important, beyond the
    /// free space.
    pub purgeable: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OtherVolume {
    pub name: String,
    /// `System`, `Preboot`, `VM` ...; none for a volume someone made.
    pub role: Option<String>,
    pub mount: Option<String>,
    pub bytes: u64,
}

/// A folder of the tree: its totals, and its folders big enough to keep.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Dir {
    /// The root's full path; the others' names.
    #[serde(rename = "n")]
    pub name: String,
    #[serde(rename = "a")]
    pub allocated: u64,
    #[serde(rename = "p")]
    pub private: u64,
    #[serde(rename = "c", default)]
    pub cold: u64,
    #[serde(rename = "k", default)]
    pub cloned: u64,
    /// Of `allocated`, what clones in it count twice (see
    /// [`scan::Totals::twice`]).
    #[serde(rename = "t", default)]
    pub twice: u64,
    /// When its bytes were modified, on average (size-weighted).
    #[serde(rename = "m", default)]
    pub mtime: Option<i64>,
    #[serde(rename = "d", default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Dir>,
}

impl Dir {
    pub fn from_node(n: &Node, root: bool) -> Dir {
        let name = if root {
            n.path.to_string_lossy().into_owned()
        } else {
            n.path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        Dir {
            name,
            allocated: n.totals.allocated,
            private: n.totals.private,
            cold: n.totals.cold,
            cloned: n.totals.cloned,
            twice: n.totals.twice,
            mtime: n.totals.mean_mtime(),
            children: n
                .children
                .iter()
                .map(|c| Dir::from_node(c, false))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppDates {
    pub path: String,
    pub last_used: Option<i64>,
    pub added: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Runtime {
    /// The folder in `/System/Library/AssetsV2` that holds it.
    pub path: String,
    /// "iOS 27.0 Simulator".
    pub name: String,
    pub last_used: Option<i64>,
}

// ---------------------------------------------------------------------------
// What is printed.

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub measured: i64,
    pub took_secs: f64,
    pub volume: VolumeSpace,
    /// Space counted as used that is freed without a clean-up: purgeable,
    /// swap, and the Trash (once it's emptied).
    pub comes_back: Vec<ComesBack>,
    /// Biggest first; together they are `volume.used`.
    pub kinds: Vec<KindReport>,
    /// Your big files nothing had opened or changed for
    /// [`clean::UNUSED_FOR`] when they were measured, biggest first. The
    /// app picks the size and how long.
    pub old_files: Vec<OldFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OldFile {
    pub path: String,
    pub name: String,
    /// Allocated.
    pub bytes: u64,
    /// Freed once it's deleted (out of the Trash too).
    pub frees: u64,
    /// When it was last read and last changed (Unix seconds). The access
    /// time misses some reads ([`clean::last_used`]): the later of the two
    /// is when it was last used, as far as can be told.
    pub accessed: i64,
    pub modified: i64,
    pub kept: bool,
    /// What `mdif clean` would do with it now: move it to the Trash.
    pub action: Option<Action>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComesBack {
    /// `purgeable`, `swap`, `update` (a macOS update waiting for a restart)
    /// or `trash`.
    pub what: &'static str,
    pub bytes: u64,
    pub how: String,
    /// The kind it's counted in, when it's known: the app draws it apart
    /// from the rest of that kind. Purgeable is in Other's "System data"
    /// when local snapshots hold it; purgeable files are wherever they are.
    pub kind: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KindReport {
    pub id: &'static str,
    pub title: &'static str,
    pub bytes: u64,
    /// What deleting all of it would free.
    pub frees: u64,
    /// Bytes in items that look like waste (unused, cold, build output,
    /// caches) and aren't kept.
    pub reclaimable: u64,
    /// Biggest first, at most [`ITEMS`].
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// As you know it (`/Users/me/...`); empty for what has no folder.
    pub path: String,
    pub name: String,
    pub bytes: u64,
    pub frees: u64,
    /// Seconds since its bytes were modified, on average (size-weighted).
    pub age: Option<i64>,
    /// When it was last opened (apps, simulator runtimes).
    pub last_used: Option<i64>,
    /// Any of `unused`, `cold`, `build`, `cache`, `kept`, `regrows`.
    pub flags: Vec<&'static str>,
    pub note: Option<String>,
    /// More than the note says, for what has no folder to look in: what's
    /// in it and why.
    pub detail: Option<String>,
    /// What `mdif clean` would do with it now.
    pub action: Option<Action>,
    /// The biggest folder in it that an app's handler knows
    /// ([`handlers`](crate::handlers)): what it is and how to make it smaller.
    pub about: Option<About>,
}

#[derive(Debug, Clone, Serialize)]
pub struct About {
    pub path: String,
    pub bytes: u64,
    pub title: &'static str,
    pub what: &'static str,
    pub how: &'static str,
    pub open: Option<crate::handlers::Open>,
    /// Emptying what in it only caches, when its handler names that.
    pub action: Option<Action>,
}

// ---------------------------------------------------------------------------
// Measuring.

/// What macOS would free by itself on the volume holding `path` (local
/// snapshots, purgeable caches, cloud files it can download again): Finder's
/// "available" less `free`, the free space now.
pub fn purgeable(path: &Path, free: u64) -> u64 {
    mdif_macos::fs::available_for_important_usage(path)
        .unwrap_or(0)
        .saturating_sub(free)
}

/// Scans the Data volume and reads what else the report needs. As long as
/// the disk map takes (`mdif map`); never downloads a cloud file.
pub fn measure(home: &Path) -> Result<Measured> {
    use mdif_macos::fs;
    let started = Instant::now();
    let measured = crate::watch::unix_now();
    fs::forbid_dataless_materialization().context("disabling cloud downloads for this process")?;
    let root = Path::new(DATA_ROOT);
    let s = scan::scan(root, &TREE).with_context(|| format!("scanning {DATA_ROOT}"))?;

    // The volumes as they are now that the scan is done.
    let st = fs::statfs(root).with_context(|| format!("statfs {DATA_ROOT}"))?;
    let (mut container, mut data) =
        crate::apfs::find_volume(crate::apfs::containers()?, &st.device)?;
    if data.capacity_in_use == 0 {
        crate::apfs::fill_missing_sizes(&mut container, &st);
        data = crate::apfs::find_volume(vec![container.clone()], &st.device)?.1;
    }
    let mounts = fs::mounts().unwrap_or_default();
    let volumes = container
        .volumes
        .iter()
        .filter(|v| v.device_identifier != data.device_identifier)
        .map(|v| OtherVolume {
            name: v.name.clone(),
            role: v.roles.first().cloned(),
            mount: mount_of(&mounts, &v.device_identifier),
            bytes: v.capacity_in_use,
        })
        .collect();
    let free = st.avail;
    let snapshots = crate::apfs::local_snapshots(Path::new(&st.mount_point))
        .map(|s| s.len())
        .unwrap_or(0);

    let mut m = Measured {
        version: VERSION,
        measured,
        took_secs: 0.0,
        home: home.to_path_buf(),
        volume: VolumeSpace {
            total: st.total,
            used: st.total.saturating_sub(free),
            free,
            purgeable: purgeable(root, free),
        },
        volumes,
        data_in_use: data.capacity_in_use,
        data_real: s.real,
        snapshots,
        unreadable_dirs: s.root.totals.unreadable_dirs,
        tree: Dir::from_node(&s.root, true),
        apps: Vec::new(),
        runtimes: Vec::new(),
        files: own_files(&s.big_files, home),
    };
    let sorted = sort(&m, &Keep::default());
    let apps: Vec<String> = sorted
        .items
        .iter()
        .filter(|i| i.what == What::App)
        .map(|i| i.path.to_string_lossy().into_owned())
        .collect();
    m.apps = app_dates(&apps);
    if sorted.items.iter().any(|i| i.what == What::Simulator) {
        m.runtimes = simulator_runtimes();
    }
    m.took_secs = started.elapsed().as_secs_f64();
    Ok(m)
}

/// After a clean-up: what changed is measured again and the kept
/// measurement patched, so the breakdown follows in a second, not after a
/// whole-disk scan. Each path is measured where the tree counts it: the
/// deepest folder it keeps at or above the path that's still there (a
/// folder deleted or moved away is measured as its parent). The volume's
/// free space is read again.
pub fn update(m: &mut Measured, paths: &[PathBuf]) -> Result<()> {
    mdif_macos::fs::forbid_dataless_materialization()
        .context("disabling cloud downloads for this process")?;
    for p in paths {
        let names: Vec<String> = match p.strip_prefix("/") {
            Ok(rel) => rel
                .iter()
                .map(|n| n.to_string_lossy().into_owned())
                .collect(),
            Err(_) => continue,
        };
        // How deep the tree goes along it, and the deepest of those that's
        // still there.
        let mut depth = 0;
        let mut d = &m.tree;
        while let Some(c) = names
            .get(depth)
            .and_then(|n| d.children.iter().find(|c| &c.name == n))
        {
            d = c;
            depth += 1;
        }
        while depth > 0 && !Path::new("/").join(names[..depth].join("/")).is_dir() {
            depth -= 1;
        }
        // Never the whole disk, a whole home folder or another folder too big
        // to measure in a moment: that's Refresh.
        if depth < 3
            || node_mut(&mut m.tree, &names[..depth]).is_none_or(|d| d.allocated > PATCH_MAX)
        {
            continue;
        }
        let at = Path::new("/").join(names[..depth].join("/"));
        let opts = scan::Options {
            max_depth: TREE.max_depth.saturating_sub(depth),
            ..TREE
        };
        let s = scan::scan(&at, &opts).with_context(|| format!("measuring {}", at.display()))?;
        let mut new = Dir::from_node(&s.root, false);
        let Some(old) = node_mut(&mut m.tree, &names[..depth]) else {
            continue;
        };
        new.name = old.name.clone();
        let delta = |n: u64, o: u64| n as i128 - o as i128;
        let (da, dp, dc, dk, dt) = (
            delta(new.allocated, old.allocated),
            delta(new.private, old.private),
            delta(new.cold, old.cold),
            delta(new.cloned, old.cloned),
            delta(new.twice, old.twice),
        );
        *old = new;
        let apply = |v: &mut u64, d: i128| *v = (*v as i128 + d).max(0) as u64;
        for up in 0..depth {
            if let Some(a) = node_mut(&mut m.tree, &names[..up]) {
                apply(&mut a.allocated, da);
                apply(&mut a.private, dp);
                apply(&mut a.cold, dc);
                apply(&mut a.cloned, dk);
                apply(&mut a.twice, dt);
            }
        }
        apply(&mut m.data_real, da - dt);
        // Its big files as they are now.
        m.files.retain(|f| !Path::new(&f.path).starts_with(&at));
        m.files.extend(own_files(&s.big_files, &m.home));
        m.files
            .sort_by(|a, b| b.allocated.cmp(&a.allocated).then(a.path.cmp(&b.path)));
        m.files.truncate(FILES);
    }
    let st = mdif_macos::fs::statfs(Path::new(DATA_ROOT))
        .with_context(|| format!("statfs {DATA_ROOT}"))?;
    let used = st.total.saturating_sub(st.avail);
    m.data_in_use = (m.data_in_use as i128 + used as i128 - m.volume.used as i128).max(0) as u64;
    m.volume.total = st.total;
    m.volume.free = st.avail;
    m.volume.used = used;
    m.volume.purgeable = purgeable(Path::new(DATA_ROOT), st.avail);
    // Snapshots deleted: what they held isn't purgeable any more.
    if paths.iter().any(|p| clean::snapshot_date(p).is_some())
        && let Ok(s) = crate::apfs::local_snapshots(Path::new(&st.mount_point))
    {
        m.snapshots = s.len();
    }
    Ok(())
}

fn node_mut<'a>(d: &'a mut Dir, names: &[String]) -> Option<&'a mut Dir> {
    let Some((first, rest)) = names.split_first() else {
        return Some(d);
    };
    let c = d.children.iter_mut().find(|c| &c.name == first)?;
    node_mut(c, rest)
}

/// Of the big files a scan found, yours ([`clean::yours`]), as you know
/// them, the biggest [`FILES`].
fn own_files(found: &[scan::BigFile], home: &Path) -> Vec<BigFile> {
    let mut files: Vec<BigFile> = found
        .iter()
        .filter_map(|f| {
            let path = shown(&f.path);
            clean::yours(&path, home).then(|| BigFile {
                path: path.to_string_lossy().into_owned(),
                allocated: f.allocated,
                private: f.private.min(f.allocated),
                accessed: f.atime,
                modified: f.mtime,
            })
        })
        .collect();
    files.sort_by(|a, b| b.allocated.cmp(&a.allocated).then(a.path.cmp(&b.path)));
    files.truncate(FILES);
    files
}

/// Where the volume `id` (`disk3s6`) is mounted: itself, or one of its
/// snapshots (the System volume starts up from `disk3s1s1`).
fn mount_of(mounts: &[(String, String)], id: &str) -> Option<String> {
    let rest = |dev: &str| {
        dev.strip_prefix("/dev/")?
            .strip_prefix(id)
            .map(str::to_owned)
    };
    let snapshot = |r: &str| {
        r.strip_prefix('s')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    mounts
        .iter()
        .find(|(dev, _)| rest(dev).is_some_and(|r| r.is_empty()))
        .or_else(|| {
            mounts
                .iter()
                .find(|(dev, _)| rest(dev).is_some_and(|r| snapshot(&r)))
        })
        .map(|(_, m)| m.clone())
}

/// When each app was last opened and added, from Spotlight's metadata (not
/// the app's files).
pub fn app_dates(apps: &[String]) -> Vec<AppDates> {
    // mdls stops at the first path that's gone.
    let apps: Vec<&String> = apps
        .iter()
        .filter(|a| std::fs::symlink_metadata(a).is_ok())
        .collect();
    let read = |attr: &str| -> Vec<Option<i64>> {
        if apps.is_empty() {
            return Vec::new();
        }
        let all = mdls(attr, &apps);
        if all.len() == apps.len() {
            return all;
        }
        // Something didn't answer: one at a time.
        apps.iter()
            .map(|a| mdls(attr, &[a]).first().copied().flatten())
            .collect()
    };
    let used = read("kMDItemLastUsedDate");
    let added = read("kMDItemDateAdded");
    apps.iter()
        .enumerate()
        .map(|(i, a)| AppDates {
            path: a.to_string(),
            last_used: used.get(i).copied().flatten(),
            added: added.get(i).copied().flatten(),
        })
        .collect()
}

fn mdls(attr: &str, paths: &[&String]) -> Vec<Option<i64>> {
    let mut cmd = Command::new("/usr/bin/mdls");
    cmd.args(["-raw", "-nullMarker", "NONE", "-name", attr])
        .args(paths);
    output_within(cmd, Duration::from_secs(20))
        .map(|out| parse_mdls(&out))
        .unwrap_or_default()
}

/// `mdls -raw` prints each file's value, separated by NULs; `NONE` for none.
pub fn parse_mdls(out: &[u8]) -> Vec<Option<i64>> {
    if out.is_empty() {
        return Vec::new();
    }
    let out = out.strip_suffix(b"\0").unwrap_or(out);
    out.split(|&b| b == 0)
        .map(|v| parse_time(String::from_utf8_lossy(v).trim()))
        .collect()
}

/// Simulator runtimes, from the Xcode that's selected.
fn simulator_runtimes() -> Vec<Runtime> {
    let Some(simctl) = simctl() else {
        return Vec::new();
    };
    let mut c = Command::new(simctl);
    c.args(["runtime", "list", "-j"]);
    output_within(c, Duration::from_secs(15))
        .map(|out| parse_runtimes(&out))
        .unwrap_or_default()
}

/// `simctl` of the Xcode that's selected, run straight from it: `xcrun`
/// without developer tools offers to install them, in a dialog. `None`
/// without Xcode (the Command Line Tools have no simulators).
pub(crate) fn simctl() -> Option<PathBuf> {
    let mut c = Command::new("/usr/bin/xcode-select");
    c.arg("-p");
    let dev = output_within(c, Duration::from_secs(5))?;
    let simctl = PathBuf::from(String::from_utf8_lossy(&dev).trim()).join("usr/bin/simctl");
    simctl.is_file().then_some(simctl)
}

/// `simctl runtime list -j`: each runtime's disk image, in an asset folder.
pub fn parse_runtimes(out: &[u8]) -> Vec<Runtime> {
    let Ok(serde_json::Value::Object(all)) = serde_json::from_slice(out) else {
        return Vec::new();
    };
    let mut found: Vec<Runtime> = all
        .values()
        .filter_map(|r| {
            let image = r["path"].as_str().or(r["parentMountPath"].as_str())?;
            let at = image.find(".asset/")? + ".asset".len();
            Some(Runtime {
                path: image[..at].to_string(),
                name: runtime_name(r),
                last_used: r["lastUsedAt"].as_str().and_then(parse_time),
            })
        })
        .collect();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// "iOS 27.0 Simulator", for a runtime as `simctl runtime list -j` lists it.
pub(crate) fn runtime_name(r: &serde_json::Value) -> String {
    let platform = r["runtimeIdentifier"]
        .as_str()
        .and_then(|id| id.rsplit('.').next())
        .and_then(|last| last.split('-').next())
        .unwrap_or("Simulator");
    let version = r["version"].as_str().unwrap_or("");
    format!("{platform} {version} Simulator")
        .replace("  ", " ")
        .trim()
        .to_string()
}

/// Its standard output, if it ends well within `wait`; killed otherwise.
fn output_within(cmd: Command, wait: Duration) -> Option<Vec<u8>> {
    run_within(cmd, wait)
        .filter(|o| o.status.success())
        .map(|o| o.stdout)
}

/// What it printed and how it ended, if it ends within `wait`; killed
/// otherwise.
pub(crate) fn run_within(mut cmd: Command, wait: Duration) -> Option<std::process::Output> {
    use std::io::Read;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let read = |mut from: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            from.read_to_end(&mut buf).map(|_| buf)
        })
    };
    let stdout = read(Box::new(child.stdout.take()?));
    let stderr = read(Box::new(child.stderr.take()?));
    let until = Instant::now() + wait;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    Some(std::process::Output {
        status,
        stdout: stdout.join().ok()?.ok()?,
        stderr: stderr.join().ok()?.unwrap_or_default(),
    })
}

// ---------------------------------------------------------------------------
// Sorting the tree into kinds.

/// What sort of item, for its flags and note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum What {
    Plain,
    /// An app's bundle.
    App,
    /// A virtual machine.
    Vm,
    /// A project's build output (`target`, `node_modules` ...).
    Artifact,
    /// `target/<profile>/incremental`.
    Incremental,
    /// A simulator runtime's asset.
    Simulator,
    /// Caches apps fill again.
    Cache,
    /// Xcode's DerivedData.
    DerivedData,
    /// A developer tool's cache (device symbols, simulator caches).
    DevCache,
    Trash,
}

impl What {
    /// Nothing inside is sorted on its own (but a target's incremental
    /// caches).
    fn sealed(self) -> bool {
        !matches!(self, What::Plain)
    }
}

/// What a rule says about a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Class {
    kind: Kind,
    /// It's an item; otherwise its bytes are its kind's, in no item.
    item: bool,
    what: What,
}

fn group(kind: Kind) -> Option<Class> {
    Some(Class {
        kind,
        item: false,
        what: What::Plain,
    })
}

fn item(kind: Kind, what: What) -> Option<Class> {
    Some(Class {
        kind,
        item: true,
        what,
    })
}

/// The item a folder is in, as its rules see it.
struct Up<'a> {
    what: What,
    item: Option<&'a Path>,
}

const VM_BUNDLES: &[&str] = &["vmwarevm", "utm", "pvm"];

/// Developer tools' folders in the home folder.
pub(crate) const DEV_HOME: &[&str] = &[
    ".gradle",
    ".cargo",
    ".rustup",
    "fvm",
    ".npm",
    ".pnpm-store",
    ".android",
    ".pub-cache",
    ".m2",
    ".nvm",
    ".bun",
    ".dartServer",
    "go",
];

/// How the folder at `path` (as you know it: `/Users/me/...`) is sorted,
/// `None` when it goes with the folder above. `up` is the item it's in;
/// `allocated`, its size.
fn classify(path: &Path, home: &Path, up: &Up, allocated: u64) -> Option<Class> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    if up.what.sealed() {
        let carved = up.what == What::Artifact
            && allocated >= INCREMENTAL_MIN
            && up.item.is_some_and(|t| incremental_in(path, t));
        return carved.then_some(Class {
            kind: Kind::Build,
            item: true,
            what: What::Incremental,
        });
    }
    let ext = Path::new(&*name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase());
    if ext.as_deref().is_some_and(|e| VM_BUNDLES.contains(&e)) {
        return item(Kind::Vms, What::Vm);
    }
    match path.strip_prefix(home) {
        Ok(rel) => in_home(&names(rel), &name),
        Err(_) => in_system(&names(path), &name),
    }
}

fn names(p: &Path) -> Vec<String> {
    p.components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

/// `target/<profile>/incremental` or `target/<triple>/<profile>/incremental`
/// of the build output `target`.
fn incremental_in(path: &Path, target: &Path) -> bool {
    path.file_name().is_some_and(|n| n == "incremental")
        && target.file_name().is_some_and(|n| n == "target")
        && path.ancestors().skip(2).take(2).any(|a| a == target)
}

fn in_home(c: &[String], name: &str) -> Option<Class> {
    use Kind::*;
    let c: Vec<&str> = c.iter().map(String::as_str).collect();
    match c.as_slice() {
        [] => group(Files),
        ["Library"] => group(Appdata),
        ["Library", "Caches"] => group(Caches),
        ["Library", "Caches", _] => item(Caches, What::Cache),
        ["Library", "Developer"] | ["Library", "Developer", "Xcode" | "CoreSimulator"] => {
            group(Dev)
        }
        ["Library", "Developer", "Xcode", "DerivedData"] => item(Dev, What::DerivedData),
        ["Library", "Developer", "Xcode", x] if x.ends_with("DeviceSupport") => {
            item(Dev, What::DevCache)
        }
        ["Library", "Developer", "CoreSimulator", "Caches"] => item(Dev, What::DevCache),
        ["Library", "Developer", _] | ["Library", "Developer", "Xcode" | "CoreSimulator", _] => {
            item(Dev, What::Plain)
        }
        ["Library", "Android"] => item(Dev, What::Plain),
        ["Library", "CloudStorage"] => group(Cloud),
        ["Library", "CloudStorage", _] | ["Library", "Mobile Documents"] => {
            item(Cloud, What::Plain)
        }
        ["Library", "Mail" | "Messages"] => item(Mail, What::Plain),
        [
            "Library",
            "Containers" | "Group Containers" | "Application Support",
        ] => group(Appdata),
        ["Library", "Containers", "com.docker.docker"]
        | ["Library", "Group Containers", "group.com.docker"] => item(Dev, What::Plain),
        ["Library", "Containers", "com.apple.mail"] => item(Mail, What::Plain),
        ["Library", "Containers", "com.utmapp.UTM"] => item(Vms, What::Vm),
        ["Library", "Group Containers", g] if g.ends_with(".com.parallels.desktop.appstore") => {
            item(Vms, What::Vm)
        }
        [
            "Library",
            "Group Containers",
            "UBF8T346G9.OneDriveSyncClientSuite",
        ] => item(Cloud, What::Plain),
        [
            "Library",
            "Containers" | "Group Containers" | "Application Support",
            _,
        ] => item(Appdata, What::Plain),
        ["Library", "Containers", _, "Data", "Library", "Caches"]
        | ["Library", "Group Containers", _, "Library", "Caches"]
        | [
            "Library",
            "Application Support",
            _,
            "Cache" | "Caches" | "Code Cache" | "GPUCache",
        ] => item(Caches, What::Cache),
        ["Library", _] => item(Appdata, What::Plain),
        [".Trash"] => item(Files, What::Trash),
        ["Pictures" | "Movies" | "Music"] => group(Media),
        ["Pictures" | "Movies" | "Music", _] => item(Media, What::Plain),
        ["Applications"] => group(Apps),
        ["Applications", ..] if name.ends_with(".app") => item(Apps, What::App),
        ["Virtual Machines.localized" | "Parallels"] => group(Vms),
        [".cache"] => group(Caches),
        [".cache", _] => item(Caches, What::Cache),
        [t] if DEV_HOME.contains(t) => item(Dev, What::Plain),
        [root] if CODE_ROOTS.contains(root) => item(Files, What::Plain),
        [root, _, .., last] if CODE_ROOTS.contains(root) && ARTIFACTS.contains(last) => {
            item(Build, What::Artifact)
        }
        [_] => item(Files, What::Plain),
        _ => None,
    }
}

fn in_system(c: &[String], name: &str) -> Option<Class> {
    use Kind::*;
    let c: Vec<&str> = c.iter().map(String::as_str).collect();
    let runtimes =
        |a: &str| a.starts_with("com_apple_MobileAsset_") && a.ends_with("SimulatorRuntime");
    match c.as_slice() {
        [] | ["Users"] | ["opt"] => group(Other),
        ["Applications"] => group(Apps),
        ["Applications", ..] if name.ends_with(".app") => item(Apps, What::App),
        ["System"]
        | ["System", "Library"]
        | ["Library"]
        | ["private"]
        | ["private", "var"]
        | ["usr"] => group(Macos),
        ["System", "Library", "AssetsV2"] => item(Macos, What::Plain),
        ["System", "Library", "AssetsV2", a] if runtimes(a) => group(Dev),
        ["System", "Library", "AssetsV2", a, _] if runtimes(a) => item(Dev, What::Simulator),
        ["Library", "Caches"] => group(Caches),
        ["Library", "Caches", _] | ["private", "var", "folders"] => item(Caches, What::Cache),
        ["Library", "Developer"] => group(Dev),
        ["Library", "Developer", _] | ["usr", "local"] | ["opt", "homebrew"] => {
            item(Dev, What::Plain)
        }
        ["Library", _] | ["private", _] | ["private", "var", _] | ["usr", _] => {
            item(Macos, What::Plain)
        }
        [
            ".Spotlight-V100"
            | ".fseventsd"
            | ".DocumentRevisions-V100"
            | "cores"
            | "MobileSoftwareUpdate"
            | ".PKInstallSandboxManager"
            | ".PKInstallSandboxManager-SystemSoftware",
        ] => item(Macos, What::Plain),
        ["Users", "Shared"] => item(Files, What::Plain),
        ["Users", _] | ["opt", _] | [_] => item(Other, What::Plain),
        _ => None,
    }
}

/// Bytes of one part of the tree: signed, since a folder's own share is its
/// total less its folders', and a hard-linked file counts once in a folder
/// but in each of two folders holding a link.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Part {
    allocated: i64,
    private: i64,
    cold: i64,
    cloned: i64,
    twice: i64,
    mtime_weighted: i128,
    /// Of `allocated`, what's in kept folders.
    kept: i64,
    /// Of `cold`, what's in folders that aren't kept.
    cold_unkept: i64,
}

impl Part {
    fn of(d: &Dir) -> Part {
        Part {
            allocated: d.allocated as i64,
            private: d.private as i64,
            cold: d.cold as i64,
            cloned: d.cloned as i64,
            twice: d.twice as i64,
            mtime_weighted: d.mtime.unwrap_or(0) as i128 * d.allocated as i128,
            kept: 0,
            cold_unkept: 0,
        }
    }

    fn add(&mut self, o: &Part) {
        self.allocated += o.allocated;
        self.private += o.private;
        self.cold += o.cold;
        self.cloned += o.cloned;
        self.twice += o.twice;
        self.mtime_weighted += o.mtime_weighted;
        self.kept += o.kept;
        self.cold_unkept += o.cold_unkept;
    }

    fn sub(&mut self, o: &Part) {
        self.allocated -= o.allocated;
        self.private -= o.private;
        self.cold -= o.cold;
        self.cloned -= o.cloned;
        self.twice -= o.twice;
        self.mtime_weighted -= o.mtime_weighted;
    }
}

/// An item as the tree sorts it.
#[derive(Debug, Clone)]
struct Sorted {
    kind: Kind,
    what: What,
    /// As you know it.
    path: PathBuf,
    /// The whole folder, as deleting it would free: what an action says.
    whole: u64,
    part: Part,
}

#[derive(Debug, Default)]
struct Tree {
    items: Vec<Sorted>,
    /// What belongs to a kind but to none of its items.
    rest: HashMap<Kind, Part>,
}

/// `path` as you know it: the Data volume's mount point left out.
fn shown(path: &Path) -> PathBuf {
    match path.strip_prefix(DATA_ROOT) {
        Ok(rest) => Path::new("/").join(rest),
        Err(_) => path.to_path_buf(),
    }
}

fn sort(m: &Measured, keep: &Keep) -> Tree {
    let mut t = Tree::default();
    let root = PathBuf::from(&m.tree.name);
    let top = Owner {
        kind: Kind::Other,
        item: None,
        what: What::Plain,
        kept: false,
    };
    visit(&m.tree, root, &top, 0, &m.home, keep, &mut t);
    t
}

#[derive(Debug, Clone, Copy)]
struct Owner {
    kind: Kind,
    item: Option<usize>,
    what: What,
    kept: bool,
}

/// `twice`: what the folder above counted twice and handed down to this one.
fn visit(d: &Dir, path: PathBuf, up: &Owner, twice: i64, home: &Path, keep: &Keep, t: &mut Tree) {
    let at = shown(&path);
    let kept = up.kept || keep.covering(&at).is_some();
    let class = {
        let item_path = up.item.map(|i| t.items[i].path.as_path());
        classify(
            &at,
            home,
            &Up {
                what: up.what,
                item: item_path,
            },
            d.allocated,
        )
    };
    let owner = match class {
        Some(c) => Owner {
            kind: c.kind,
            item: c.item.then(|| {
                t.items.push(Sorted {
                    kind: c.kind,
                    what: c.what,
                    path: at.clone(),
                    whole: d.allocated,
                    part: Part::default(),
                });
                t.items.len() - 1
            }),
            what: c.what,
            kept,
        },
        None => Owner { kept, ..*up },
    };
    let mut own = Part::of(d);
    for c in &d.children {
        own.sub(&Part::of(c));
    }
    // What's counted twice here and not inside one of its folders (blocks
    // its folders or files share with each other) goes to them, by what
    // they share.
    let across = (own.twice + twice).max(0) as i128;
    let weight = |c: &Dir| c.cloned.saturating_sub(c.twice) as i128;
    let weights = own.cloned.max(0) as i128 + d.children.iter().map(weight).sum::<i128>();
    let handed: Vec<i64> = d
        .children
        .iter()
        .map(|c| match weights {
            0 => 0,
            w => (across * weight(c) / w).min(weight(c)) as i64,
        })
        .collect();
    own.twice = (across - handed.iter().map(|&h| h as i128).sum::<i128>()) as i64;
    if kept {
        own.kept = own.allocated;
    } else {
        own.cold_unkept = own.cold;
    }
    match owner.item {
        Some(i) => t.items[i].part.add(&own),
        None => t.rest.entry(owner.kind).or_default().add(&own),
    }
    for (c, h) in d.children.iter().zip(handed) {
        visit(c, path.join(&c.name), &owner, h, home, keep, t);
    }
}

/// Sorts what `m` measured into kinds, with what you keep now (`keep`) and
/// the local snapshots there are now (`snapshots`, as `tmutil
/// listlocalsnapshots` gives them). Actions are offered as `mdif clean`
/// would take them now.
pub fn report(m: &Measured, keep: &Keep, snapshots: &[String]) -> Report {
    let t = sort(m, keep);
    let home = m.home.as_path();

    // Clones counted once: take off what each part's clones count twice,
    // then what's still counted twice (blocks shared between parts, hard
    // links) where the clones are, down to what the scan found really used.
    let total: i128 = t
        .items
        .iter()
        .map(|i| i.part.allocated as i128)
        .sum::<i128>()
        + t.rest.values().map(|p| p.allocated as i128).sum::<i128>();
    let real = m.data_real.min(m.data_in_use) as i128;
    let parts = || t.items.iter().map(|i| &i.part).chain(t.rest.values());
    let shared = |p: &Part| (p.allocated - p.private).max(0) as i128;
    let twice = |p: &Part| (p.twice.max(0) as i128).min(shared(p));
    let over = (total - real - parts().map(twice).sum::<i128>()).max(0);
    let weight: fn(&Part) -> i128 = if parts().any(|p| p.cloned > 0) {
        |p| (p.cloned - p.twice).max(0) as i128
    } else {
        |p| (p.allocated - p.private).max(0) as i128
    };
    let weights: i128 = parts().map(weight).sum();
    let size = |p: &Part| -> (u64, u64) {
        let cut = twice(p)
            + if weights > 0 {
                (over * weight(p) / weights).min(shared(p) - twice(p))
            } else {
                0
            };
        let bytes = (p.allocated as i128 - cut).max(0) as u64;
        (bytes, (p.private.max(0) as u64).min(bytes))
    };

    let dates: HashMap<&str, &AppDates> = m.apps.iter().map(|a| (a.path.as_str(), a)).collect();
    let mut kinds: HashMap<Kind, (Vec<Item>, u64, u64, u64)> = HashMap::new();
    let mut trash = 0;
    for s in &t.items {
        let (bytes, frees) = size(&s.part);
        if s.what == What::Trash {
            trash += bytes;
        }
        let it = describe(s, bytes, frees, m, keep, &dates);
        let reclaim = reclaimable(s, &it, bytes);
        let k = kinds.entry(s.kind).or_default();
        k.1 += bytes;
        k.2 += frees;
        k.3 += reclaim;
        k.0.push(it);
    }
    for (kind, p) in &t.rest {
        let (bytes, frees) = size(p);
        let k = kinds.entry(*kind).or_default();
        k.1 += bytes;
        k.2 += frees;
        // Caches too small to list are caches all the same.
        if *kind == Kind::Caches {
            k.3 += scaled(bytes, p.allocated - p.kept, p.allocated);
        }
    }

    // The other volumes.
    for v in &m.volumes {
        if v.bytes == 0 {
            continue;
        }
        let (kind, name, note) = match v.role.as_deref() {
            Some("System") => (
                Kind::Macos,
                "macOS".to_string(),
                Some("macOS itself, read-only"),
            ),
            Some("VM") => (
                Kind::Macos,
                "Swap".into(),
                Some("Removed when the Mac restarts"),
            ),
            Some("Preboot") => (
                Kind::Macos,
                "Preboot".into(),
                Some("What the Mac starts up from"),
            ),
            Some("Recovery") => (Kind::Macos, "Recovery".into(), Some("macOS Recovery")),
            Some("Update") => (
                Kind::Macos,
                "Update".into(),
                Some("A macOS update being installed"),
            ),
            Some(role) => (Kind::Macos, role.to_string(), None),
            None => (
                Kind::Other,
                v.name.clone(),
                Some("Another volume on this disk"),
            ),
        };
        let k = kinds.entry(kind).or_default();
        k.1 += v.bytes;
        k.0.push(Item {
            path: v.mount.clone().unwrap_or_default(),
            name,
            bytes: v.bytes,
            frees: 0,
            age: None,
            last_used: None,
            flags: Vec::new(),
            note: note.map(String::from),
            detail: None,
            action: None,
            about: None,
        });
    }

    // What the scan couldn't see, so the kinds add up to what's used.
    let counted: u64 = kinds.values().map(|k| k.1).sum();
    let unmeasured = m.volume.used as i128 - counted as i128;
    let back = ComingBack {
        swap: m
            .volumes
            .iter()
            .filter(|v| v.role.as_deref() == Some("VM"))
            .map(|v| v.bytes)
            .sum(),
        trash,
    };
    if unmeasured > 0 {
        let k = kinds.entry(Kind::Other).or_default();
        k.1 += unmeasured as u64;
        k.0.push(unseen(unmeasured as u64, m, snapshots));
    } else if unmeasured < 0 {
        // Counted more than is used (the disk changed while it was
        // scanned): taken off the biggest kind.
        if let Some(k) = kinds.values_mut().max_by_key(|k| k.1) {
            k.1 = k.1.saturating_sub(unmeasured.unsigned_abs() as u64);
            k.2 = k.2.min(k.1);
            k.3 = k.3.min(k.1);
        }
    }

    let mut out: Vec<KindReport> = Kind::ALL
        .iter()
        .filter_map(|kind| {
            let (mut items, bytes, frees, reclaimable) = kinds.remove(kind)?;
            if bytes == 0 {
                return None;
            }
            items.retain(|i| i.bytes > 0);
            items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.path.cmp(&b.path)));
            items.truncate(ITEMS);
            // System data comes with its own (the snapshots').
            for i in items.iter_mut().filter(|i| i.action.is_none()) {
                i.action = action(i, home, keep, &t);
            }
            Some(KindReport {
                id: kind.id(),
                title: kind.title(),
                bytes,
                frees,
                reclaimable: reclaimable.min(bytes),
                items,
            })
        })
        .collect();
    out.sort_by_key(|k| std::cmp::Reverse(k.bytes));

    Report {
        measured: m.measured,
        took_secs: m.took_secs,
        volume: m.volume,
        comes_back: comes_back(m, &back),
        kinds: out,
        old_files: old_files(m, keep),
    }
}

/// What the scan couldn't see, `bytes` of it, said in full: what Time
/// Machine's local snapshots (of `snapshots`) hold, up to what macOS counts
/// as purgeable, with the button that deletes them; and what macOS keeps
/// where apps aren't let in, which nothing can measure or clean.
fn unseen(bytes: u64, m: &Measured, snapshots: &[String]) -> Item {
    let folders = format!(
        "{} macOS keeps to itself (Spotlight's index, system logs, caches and databases) and \
         the file system's own records. Apps aren't let in to measure or clean them: macOS \
         looks after them itself.",
        match m.unreadable_dirs {
            0 => "folders".to_string(),
            1 => "a folder".to_string(),
            n => format!("{n} folders"),
        }
    );
    let action = clean::snapshots_action(snapshots);
    let (name, note, detail) = match &action {
        None => (
            "System data",
            "Folders only macOS can read, and file-system metadata".to_string(),
            format!("In {folders}"),
        ),
        Some(a) => {
            let one = a.paths.len() == 1;
            let oldest = a
                .paths
                .iter()
                .filter_map(|p| clean::snapshot_date(Path::new(p)))
                .min()
                .map(clean::snapshot_when)
                .unwrap_or_default();
            let them = if one {
                "1 Time Machine snapshot".to_string()
            } else {
                format!("{} Time Machine snapshots", a.paths.len())
            };
            let held = m.volume.purgeable.min(bytes);
            let note = if held > 0 {
                format!(
                    "Up to {} in {them}, the rest in folders only macOS can read",
                    crate::units::bytes(held)
                )
            } else {
                format!("{them}, and folders only macOS can read")
            };
            let (it, they, their, keeps) = if one {
                ("it", "it", "its", "keeps")
            } else {
                ("they", "them", "their", "keep")
            };
            let mut detail = format!(
                "{} {oldest}, {keeps} every file that was there when {it} {} taken: a file \
                 deleted since frees nothing until {it} {}. macOS deletes {they} after a day, or \
                 sooner when it needs the room{}. Deleting {they} now frees {their} space at \
                 once; backups on your backup disk aren't touched.",
                if one {
                    "A local Time Machine snapshot, from".to_string()
                } else {
                    format!(
                        "{} local Time Machine snapshots, the oldest from",
                        a.paths.len()
                    )
                },
                if one { "was" } else { "were" },
                if one { "goes" } else { "go" },
                if held > 0 {
                    format!(
                        ", and counts what {it} {} as purgeable: up to {}",
                        if one { "holds" } else { "hold" },
                        crate::units::bytes(held)
                    )
                } else {
                    String::new()
                },
            );
            if held < bytes {
                detail.push_str(&format!(
                    "\n\nThe rest, {}, is in {folders}",
                    crate::units::bytes(bytes - held)
                ));
            }
            ("Snapshots and system data", note, detail)
        }
    };
    Item {
        path: String::new(),
        name: name.into(),
        bytes,
        frees: 0,
        age: None,
        last_used: None,
        flags: Vec::new(),
        note: Some(note),
        detail: Some(detail),
        action,
        about: None,
    }
}

/// Your big files nothing had opened or changed for [`clean::UNUSED_FOR`]
/// when `m` was measured, biggest first, each with what `mdif clean` would
/// do with it now. Those gone since (moved to the Trash) are left out.
fn old_files(m: &Measured, keep: &Keep) -> Vec<OldFile> {
    let gone = |path: &Path| {
        crate::privacy::may_open(path)
            && std::fs::symlink_metadata(path)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    };
    let mut out: Vec<OldFile> = m
        .files
        .iter()
        .filter(|f| m.measured - clean::last_used(f.accessed, f.modified) >= clean::UNUSED_FOR)
        .filter(|f| !gone(Path::new(&f.path)))
        .map(|f| {
            let path = Path::new(&f.path);
            let kept = keep.covering(path).is_some();
            let trash = !kept
                && crate::privacy::may_open(path)
                && matches!(
                    clean::plan(path, &m.home, keep, &[], &[]),
                    Ok(Step::Trash { quit: None, .. })
                );
            OldFile {
                path: f.path.clone(),
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                bytes: f.allocated,
                frees: f.private.min(f.allocated),
                accessed: f.accessed,
                modified: f.modified,
                kept,
                action: trash.then(|| Action {
                    kind: "trash",
                    label: "Move to Trash".into(),
                    paths: vec![f.path.clone()],
                    confirm: None,
                    done: None,
                }),
            }
        })
        .collect();
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.path.cmp(&b.path)));
    out
}

/// `bytes` times `num / den`, for a part of something scaled.
fn scaled(bytes: u64, num: i64, den: i64) -> u64 {
    if den <= 0 || num <= 0 {
        return 0;
    }
    (bytes as u128 * num.min(den) as u128 / den as u128) as u64
}

/// Of an item, what looks like waste and isn't kept.
fn reclaimable(s: &Sorted, it: &Item, bytes: u64) -> u64 {
    if it.flags.contains(&"kept") {
        return 0;
    }
    let p = &s.part;
    if it
        .flags
        .iter()
        .any(|f| matches!(*f, "unused" | "build" | "cache"))
    {
        scaled(bytes, p.allocated - p.kept, p.allocated)
    } else if s.kind.cold_counts() {
        scaled(bytes, p.cold_unkept, p.allocated)
    } else {
        0
    }
}

fn describe(
    s: &Sorted,
    bytes: u64,
    frees: u64,
    m: &Measured,
    keep: &Keep,
    dates: &HashMap<&str, &AppDates>,
) -> Item {
    let path = s.path.to_string_lossy().into_owned();
    let p = &s.part;
    let mean = (p.allocated > 0).then(|| (p.mtime_weighted / p.allocated as i128) as i64);
    let mut flags: Vec<&'static str> = Vec::new();
    let mut note: Option<String> = None;
    let mut last_used = None;
    match s.what {
        What::App => {
            let d = dates.get(path.as_str());
            last_used = d.and_then(|d| d.last_used);
            let added = d.and_then(|d| d.added);
            match (last_used, added) {
                (Some(u), _) if m.measured - u > UNUSED => {
                    flags.push("unused");
                    note = Some(format!("Not opened since {}", month(u, m.measured)));
                }
                (None, Some(a)) if m.measured - a > UNUSED => {
                    flags.push("unused");
                    note = Some("Never opened".into());
                }
                _ => {}
            }
        }
        What::Artifact => {
            flags.push("build");
            note = Some(artifact_note(&s.path));
        }
        What::Incremental => {
            flags.push("build");
            note = Some(
                "Rust's incremental caches: only the next build of your own crates is slower"
                    .into(),
            );
        }
        What::DerivedData => {
            flags.push("build");
            note = Some("Xcode's build output: the next build makes it again".into());
        }
        What::Cache => {
            flags.extend(["cache", "regrows"]);
            note = Some(if s.path == Path::new("/private/var/folders") {
                "Apps' temporary files and caches: macOS clears out old ones itself".into()
            } else if s.path.starts_with(&m.home) && clean::cache_of(&s.path, &m.home).is_none() {
                "macOS's own, or shared by several apps: left to macOS".into()
            } else {
                "Apps fill it again as they run".into()
            });
        }
        What::DevCache => {
            flags.push("cache");
            note = Some("The developer tools make it again when needed".into());
        }
        What::Simulator => {
            let r = m.runtimes.iter().find(|r| Path::new(&r.path) == s.path);
            last_used = r.and_then(|r| r.last_used);
            note = Some(
                "Xcode → Settings → Components removes it; it downloads again when needed".into(),
            );
        }
        What::Vm => {
            note = Some("A virtual machine's disk: it grows as the guest writes".into());
        }
        What::Trash => note = Some("Freed when you empty the Trash".into()),
        What::Plain => note = plain_note(&s.path, &m.home),
    }
    let cold_share = if p.allocated > 0 {
        p.cold.max(0) as f64 / p.allocated as f64
    } else {
        0.0
    };
    let cold = s.kind.cold_counts() && cold_share >= COLD_SHARE;
    // Which folder in it holds most of it, said first: "Mostly Outlook ›
    // Outlook 15 Profiles › Main Profile › Data, 4.9 GB: ...".
    // A game's folder, by the file only it has (reading names, not files).
    if matches!(s.what, What::Plain)
        && note.is_none()
        && crate::privacy::may_open(&s.path)
        && s.path.join("launcher_profiles.json").is_file()
    {
        note = Some(minecraft_note(&s.path, &m.home));
    }
    let about = about(m, &s.path, bytes);
    // What the handler says of the whole item, the box says: no note too.
    if about.as_ref().is_some_and(|a| Path::new(&a.path) == s.path) {
        note = None;
    }
    if matches!(s.what, What::Plain)
        && about.is_none()
        && let Some((inside, size)) = mostly(m, &s.path, bytes, cold)
    {
        let t = format!("Mostly {inside}, {}", crate::units::bytes(size));
        note = Some(match note {
            Some(n) => format!("{n}. {t}"),
            None => t,
        });
    }
    if cold {
        flags.push("cold");
        // The app shows how long in a pill; said here only when nothing else is.
        if note.is_none()
            && let Some(t) = mean
        {
            note = Some(format!("Unchanged since {}", civil(t).0));
        }
    }
    if keep.covering(&s.path).is_some() {
        flags.push("kept");
    }
    Item {
        name: name(s, m),
        path,
        bytes,
        frees,
        age: mean.map(|t| (m.measured - t).max(0)),
        last_used,
        flags,
        note,
        detail: None,
        action: None,
        about,
    }
}

/// What a Minecraft game folder holds, its worlds by name, and whether
/// Minecraft is still on this Mac.
fn minecraft_note(dir: &Path, home: &Path) -> String {
    let mut t = format!(
        "Minecraft's game folder: the game, what it downloaded, and {}",
        worlds(dir)
    );
    if dir.file_name() != Some("minecraft".as_ref()) {
        t.push_str(". Minecraft normally keeps it in `~/Library/Application Support/minecraft`");
    }
    let installed = [
        PathBuf::from("/Applications/Minecraft.app"),
        PathBuf::from("/Applications/Minecraft Launcher.app"),
        home.join("Applications/Minecraft.app"),
    ]
    .iter()
    .any(|p| p.exists());
    if !installed {
        t.push_str(". Minecraft isn't installed any more");
    }
    t
}

/// "6 saved worlds (Island, New World, …)".
fn worlds(dir: &Path) -> String {
    let w = clean::minecraft_worlds(dir);
    let names = w
        .iter()
        .take(3)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    match w.len() {
        0 => "no saved worlds".into(),
        1 => format!("one saved world ({names})"),
        n if n <= 3 => format!("{n} saved worlds ({names})"),
        n => format!("{n} saved worlds ({names}, …)"),
    }
}

/// `, the folder named “Application”, not “Application Support”,` when a
/// folder beside `dir` starts with its name and a space: a launcher that
/// split `Application Support` at the space made `~/Library/Application`,
/// and its path alone reads as cut short.
fn not_to_be_confused(dir: &Path) -> String {
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
        return String::new();
    };
    let name = name.to_string_lossy();
    let mut longer: Vec<String> = std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            n.strip_prefix(name.as_ref())
                .is_some_and(|r| r.starts_with(' '))
        })
        .map(|n| format!("“{}”", crate::text::printable(&n)))
        .collect();
    if longer.is_empty() {
        return String::new();
    }
    longer.sort();
    longer.truncate(3);
    format!(
        ", the folder named “{}”, not {},",
        crate::text::printable(&name),
        longer.join(" or ")
    )
}

fn artifact_note(path: &Path) -> String {
    let marker = |m: &str| path.parent().is_some_and(|p| p.join(m).is_file());
    match path.file_name().and_then(|n| n.to_str()).unwrap_or("") {
        "target" if marker("Cargo.toml") => "Rust build output".into(),
        "node_modules" => "Packages npm installs again".into(),
        ".venv" => "A Python environment, made again from its requirements".into(),
        "Pods" => "Pods CocoaPods installs again".into(),
        "DerivedData" => "Xcode's build output".into(),
        ".dart_tool" => "Dart's tool caches".into(),
        "build" if marker("pubspec.yaml") => "Flutter build output".into(),
        _ => "Build output: the next build makes it again".into(),
    }
}

/// An app's container or group container (`UBF8T346G9.Office`,
/// `group.com.apple.VoiceMemos.shared`, `com.microsoft.teams2`) by the app's
/// name, as far as it's known; otherwise without the team ID and `group.`.
fn container_name(c: &str) -> String {
    const KNOWN: &[(&str, &str)] = &[
        ("UBF8T346G9.Office", "Microsoft Office (shared)"),
        ("UBF8T346G9.OfficeOsfWebHost", "Office add-ins"),
        ("UBF8T346G9.ms", "Microsoft (shared)"),
        ("UBF8T346G9.com.microsoft.teams", "Microsoft Teams"),
        ("com.microsoft.teams2", "Microsoft Teams"),
        ("com.microsoft.Outlook", "Outlook"),
        ("com.microsoft.Word", "Word"),
        ("com.microsoft.Excel", "Excel"),
        ("com.microsoft.Powerpoint", "PowerPoint"),
        ("group.com.apple.VoiceMemos.shared", "Voice Memos"),
        ("group.com.apple.notes", "Notes"),
        ("group.com.apple.calendar", "Calendar"),
        ("com.apple.Safari", "Safari"),
        ("com.apple.mediaanalysisd", "Photo analysis (macOS)"),
        ("com.apple.photoanalysisd", "Photo analysis (macOS)"),
        ("com.apple.Notes", "Notes"),
        ("com.apple.news", "News"),
        ("com.apple.podcasts", "Podcasts"),
        ("com.apple.Maps", "Maps"),
        ("com.apple.FinalCut", "Final Cut Pro"),
        ("com.apple.iMovieApp", "iMovie"),
        ("com.spotify.client", "Spotify"),
        ("ru.keepcoder.Telegram", "Telegram"),
        ("net.whatsapp.WhatsApp", "WhatsApp"),
    ];
    // A team ID: ten capitals and digits.
    let team = |s: &str| {
        s.len() == 10
            && s.bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    };
    let bare = match c.split_once('.') {
        Some((t, rest)) if team(t) => rest,
        _ => c,
    };
    let bare = bare.strip_prefix("group.").unwrap_or(bare);
    KNOWN
        .iter()
        .find(|(k, _)| *k == c || *k == bare)
        .map(|(_, n)| (*n).to_string())
        .unwrap_or_else(|| bare.to_string())
}

/// The item's folder in the tree.
fn dir_of<'a>(m: &'a Measured, item: &Path) -> Option<&'a Dir> {
    let mut d = &m.tree;
    for name in item.strip_prefix("/").ok()?.iter() {
        d = d.children.iter().find(|c| *c.name == *name)?;
    }
    Some(d)
}

/// The biggest folder in an item (or the item) that an app's handler knows.
fn about(m: &Measured, item: &Path, bytes: u64) -> Option<About> {
    fn walk(
        d: &Dir,
        path: PathBuf,
        home: &Path,
        depth: usize,
        best: &mut Option<(u64, PathBuf, &'static crate::handlers::Handler)>,
    ) {
        if let Ok(rel) = path.strip_prefix(home)
            && let Some(h) = crate::handlers::find(rel)
            && best.as_ref().is_none_or(|(b, _, _)| d.allocated > *b)
        {
            *best = Some((d.allocated, path.clone(), h));
        }
        if depth < 8 {
            for c in &d.children {
                walk(c, path.join(&c.name), home, depth + 1, best);
            }
        }
    }
    let d = dir_of(m, item)?;
    let mut best = None;
    walk(d, item.to_path_buf(), &m.home, 0, &mut best);
    let (size, path, h) = best?;
    let size = (size as u128 * bytes as u128 / d.allocated.max(1) as u128) as u64;
    // A small Photos library needs no advice.
    if size < 200_000_000 {
        return None;
    }
    Some(About {
        path: path.to_string_lossy().into_owned(),
        // In the item's own terms: shared bytes taken off as the item was.
        bytes: size,
        title: h.title,
        what: h.what,
        how: h.how,
        open: h.open,
        action: empty_action(h, &path, &m.home).or_else(|| trash_action(h, &path, &m.home, size)),
    })
}

/// The button for a folder its handler says can go to the Trash.
fn trash_action(h: &crate::handlers::Handler, at: &Path, home: &Path, size: u64) -> Option<Action> {
    if !h.trash {
        return None;
    }
    let Ok(Step::Trash { .. }) = clean::plan(at, home, &Keep::default(), &[], &[]) else {
        return None;
    };
    Some(Action {
        kind: "trash",
        label: "Move to Trash".into(),
        paths: vec![at.to_string_lossy().into_owned()],
        confirm: Some(format!(
            "Move {} to the Trash? {}. Check first that what you need in it is somewhere else; \
             Put Back returns it, and its space comes back once you empty the Trash.",
            display(at, home),
            bytes(size)
        )),
        done: None,
    })
}

/// The button for a handler's caches: those of its folders to empty that
/// are there now and that `mdif clean` would empty.
fn empty_action(h: &crate::handlers::Handler, base: &Path, home: &Path) -> Option<Action> {
    fn expand(at: PathBuf, rest: &[&str], out: &mut Vec<PathBuf>) {
        let Some((first, rest)) = rest.split_first() else {
            if std::fs::symlink_metadata(&at).is_ok_and(|m| m.is_dir()) {
                out.push(at);
            }
            return;
        };
        if *first == "*" {
            if let Ok(entries) = std::fs::read_dir(&at) {
                for e in entries.flatten() {
                    expand(e.path(), rest, out);
                }
            }
        } else {
            expand(at.join(first), rest, out);
        }
    }
    if h.empty.is_empty() || !crate::privacy::may_open(base) {
        return None;
    }
    let mut found = Vec::new();
    for e in h.empty {
        expand(
            base.to_path_buf(),
            &e.split('/').collect::<Vec<_>>(),
            &mut found,
        );
    }
    found.retain(|p| {
        matches!(
            clean::plan(p, home, &Keep::default(), &[], &[]),
            Ok(Step::Empty { .. })
        )
    });
    let size: u64 = found.iter().filter_map(|p| scan::size_of(p)).sum();
    if found.is_empty() || size < 50_000_000 {
        return None;
    }
    let app = h
        .open
        .and_then(|o| o.label.strip_prefix("Open "))
        .unwrap_or("the app");
    Some(Action {
        kind: "empty",
        label: format!("Empty Caches ({})", bytes(size)),
        paths: found
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        confirm: Some(format!(
            "Empty {app}'s caches? {} on disk. Only what downloads again is deleted, not moved \
             to the Trash; quit {app} first.",
            bytes(size)
        )),
        done: None,
    })
}

/// Where most of a forgotten item is, when one folder in it holds most of
/// it: its path from the item (at most three folders) and its size. Says
/// what's in what nobody remembers; "Mostly versions" elsewhere says nothing.
fn mostly(m: &Measured, item: &Path, bytes: u64, forgotten: bool) -> Option<(String, u64)> {
    const SHARE: f64 = 0.6;
    const DEPTH: usize = 3;
    if !forgotten {
        return None;
    }
    let generic = |n: &str| n.len() >= 32 && n.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    let mut d = dir_of(m, item)?;
    let top = d.allocated;
    let mut path: Vec<&str> = Vec::new();
    while path.len() < DEPTH
        && let Some(c) = d
            .children
            .iter()
            .find(|c| c.allocated as f64 >= SHARE * d.allocated as f64 && !generic(&c.name))
    {
        path.push(&c.name);
        d = c;
    }
    if path.is_empty() || d.allocated < 1_000_000_000 {
        return None;
    }
    let size = (d.allocated as u128 * bytes as u128 / top.max(1) as u128) as u64;
    Some((path.join(" › "), size))
}

/// A note for a folder known by its place.
fn plain_note(path: &Path, home: &Path) -> Option<String> {
    let note = match path.strip_prefix(home).ok().and_then(|r| r.to_str()) {
        Some("Library/Containers/com.docker.docker") => {
            "Docker's disk image: `docker system prune` frees what no container uses"
        }
        Some("Library/Group Containers/UBF8T346G9.OneDriveSyncClientSuite") => {
            "OneDrive's copy of what it keeps on this Mac"
        }
        Some("Library/Mobile Documents") => {
            "Downloaded iCloud files: Remove Download keeps them in iCloud only"
        }
        Some("Library/Metadata") => "Spotlight's index of what's in your apps",
        Some("Library/Developer/CoreSimulator/Devices") => {
            "Simulators' apps and data: `xcrun simctl delete unavailable` removes those of \
             runtimes that are gone"
        }
        Some(rel) if rel.starts_with("Library/CloudStorage/") => {
            "Downloaded cloud files: Free Up Space keeps them in the cloud only"
        }
        _ => match path.to_str() {
            Some("/System/Library/AssetsV2") => "Voices, dictionaries and models macOS downloads",
            Some("/opt/homebrew") => {
                "What Homebrew installed. It removes old versions by itself every 30 days, so \
                 this is what's installed now: `brew leaves` lists what you asked for, and \
                 `brew uninstall` removes one"
            }
            Some("/.Spotlight-V100") => "Spotlight's index",
            Some("/private/var/vm") => "Where the Mac keeps memory while it sleeps",
            _ => return None,
        },
    };
    Some(note.to_string())
}

/// What an item is called.
fn name(s: &Sorted, m: &Measured) -> String {
    let last = s
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = |ext: &str| {
        last.strip_suffix(ext)
            .map(str::to_string)
            .unwrap_or(last.clone())
    };
    let rel = s.path.strip_prefix(&m.home).ok();
    match s.what {
        What::App => return stem(".app"),
        What::Vm => {
            return Path::new(&last)
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(last);
        }
        // Where in the code folder: `app/src-tauri/target`.
        What::Artifact | What::Incremental => {
            if let Some(rel) = rel {
                return rel
                    .iter()
                    .skip(1)
                    .map(|n| n.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
            }
        }
        What::Simulator => {
            return m
                .runtimes
                .iter()
                .find(|r| Path::new(&r.path) == s.path)
                .map(|r| r.name.clone())
                .unwrap_or_else(|| {
                    let platform = s
                        .path
                        .parent()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let platform = platform
                        .trim_start_matches("com_apple_MobileAsset_")
                        .trim_end_matches("SimulatorRuntime");
                    format!("{platform} Simulator").trim().to_string()
                });
        }
        What::Trash => return "Trash".into(),
        What::Cache => {
            // An app's cache is called after the app.
            if let Some(rel) = rel {
                let parts: Vec<_> = rel.iter().map(|n| n.to_string_lossy()).collect();
                if parts.len() > 3
                    && parts[0] == "Library"
                    && matches!(
                        &*parts[1],
                        "Containers" | "Group Containers" | "Application Support"
                    )
                {
                    return container_name(&parts[2]);
                }
            }
            if s.path == Path::new("/private/var/folders") {
                return "Temporary files".into();
            }
        }
        _ => {}
    }
    if let Some(rel) = rel
        && let ["Library", "Containers" | "Group Containers", c] = rel
            .iter()
            .map(|n| n.to_str().unwrap_or(""))
            .collect::<Vec<_>>()
            .as_slice()
        && !matches!(
            *c,
            "UBF8T346G9.OneDriveSyncClientSuite" | "com.docker.docker" | "group.com.docker"
        )
        && *c != "com.apple.mail"
    {
        return container_name(c);
    }
    let known = match rel.and_then(|r| r.to_str()) {
        Some("Library/Mobile Documents") => "iCloud Drive",
        Some("Library/Group Containers/UBF8T346G9.OneDriveSyncClientSuite") => "OneDrive",
        Some("Library/Containers/com.docker.docker") => "Docker",
        Some("Library/Group Containers/group.com.docker") => "Docker (shared)",
        Some("Library/Containers/com.apple.mail") => "Mail downloads",
        Some("Library/Android") => "Android SDK",
        Some(".android") => "Android emulators",
        Some(".gradle") => "Gradle",
        Some(".cargo") => "Cargo",
        Some(".rustup") => "Rust toolchains",
        Some("fvm") => "Flutter versions",
        Some(".npm") => "npm",
        Some(".pnpm-store") => "pnpm",
        Some(".pub-cache") => "Dart packages",
        Some("Library/Developer/CoreSimulator/Devices") => "Simulator devices",
        Some("Library/Developer/Xcode/DerivedData") => "Xcode DerivedData",
        Some("Library/Developer/Xcode/Archives") => "Xcode archives",
        Some("Library/Metadata") => "App search index",
        _ => match s.path.to_str() {
            Some("/System/Library/AssetsV2") => "System downloads",
            Some("/.Spotlight-V100") => "Spotlight index",
            Some("/.DocumentRevisions-V100") => "Document versions",
            Some("/.fseventsd") => "File-system events",
            Some("/opt/homebrew") => "Homebrew",
            Some("/private/var/vm") => "Sleep image",
            Some("/Library/Developer/CommandLineTools") => "Command Line Tools",
            _ => "",
        },
    };
    if !known.is_empty() {
        return known.into();
    }
    // A system folder by its path: `/private/var/db`, `/usr/local`.
    if rel.is_none() && s.what == What::Plain && s.kind != Kind::Files {
        return s.path.to_string_lossy().into_owned();
    }
    if s.kind == Kind::Media {
        return stem(".photoslibrary");
    }
    last
}

/// What `mdif clean` would do with a listed item now: build output, a Rust
/// build's incremental caches and an app's cache, unless kept.
fn action(i: &Item, home: &Path, keep: &Keep, t: &Tree) -> Option<Action> {
    let s = t
        .items
        .iter()
        .find(|s| s.path.as_os_str() == i.path.as_str())?;
    let shown = display(&s.path, home);
    if matches!(s.what, What::Plain)
        && crate::privacy::may_open(&s.path)
        && clean::minecraft(&s.path)
        && !keep.touches(&s.path)
    {
        let Ok(Step::Trash { .. }) = clean::plan(&s.path, home, keep, &[], &[]) else {
            return None;
        };
        return Some(Action {
            kind: "trash",
            label: "Move to Trash".into(),
            paths: vec![i.path.clone()],
            confirm: Some(format!(
                "Move {shown}{} to the Trash? {}: Minecraft's game, what it downloaded, and {}. \
                 To keep the worlds, copy its saves folder somewhere else first. Put Back \
                 returns all of it; the space comes back once you empty the Trash.",
                not_to_be_confused(&s.path),
                bytes(s.whole),
                worlds(&s.path)
            )),
            done: None,
        });
    }
    if !matches!(s.what, What::Artifact | What::Incremental | What::Cache) || keep.touches(&s.path)
    {
        return None;
    }
    match clean::plan(&s.path, home, keep, &[], &[]) {
        Ok(Step::Delete(_)) => {}
        // Whether its app is open is asked when it's done: this is cached.
        Ok(Step::Empty { .. }) => {
            return Some(Action {
                kind: "empty",
                label: "Empty Cache".into(),
                paths: vec![i.path.clone()],
                confirm: Some(format!(
                    "Empty {shown}? {} on disk. What's in it is deleted, not moved to the \
                     Trash; the app makes what it needs again, so it may be slower for a while \
                     and download some of it again.",
                    bytes(s.whole)
                )),
                done: None,
            });
        }
        _ => return None,
    }
    let (label, confirm) = if s.path.file_name() == Some("node_modules".as_ref()) {
        (
            "Delete node_modules",
            format!(
                "Delete {shown}? {} on disk. It's deleted, not moved to the Trash; `npm \
                 install` (or the project's package manager) puts it back as its lockfile says, \
                 downloading what isn't in its cache.",
                bytes(s.whole)
            ),
        )
    } else if s.what == What::Incremental {
        (
            "Delete Incremental Caches",
            format!(
                "Delete the incremental caches in {shown}? {} on disk. They're deleted, not \
                 moved to the Trash; only the next build of your own crates is slower.",
                bytes(s.whole)
            ),
        )
    } else {
        (
            "Delete Build Output",
            format!(
                "Delete {shown}? {} on disk. It's deleted, not moved to the Trash; the next \
                 build makes it again.",
                bytes(s.whole)
            ),
        )
    };
    Some(Action {
        kind: "delete",
        label: label.into(),
        paths: vec![i.path.clone()],
        confirm: Some(confirm),
        done: None,
    })
}

struct ComingBack {
    swap: u64,
    trash: u64,
}

/// What the disk is made of, in layers for the history (`strata`): each
/// kind, with the Trash taken out of your files and what the scan couldn't
/// see (Time Machine's local snapshots, macOS's own files, metadata) out of
/// Other. Together they are what's used.
pub fn strata(r: &Report) -> Vec<(&'static str, u64)> {
    let item = |kind: &str, f: &dyn Fn(&Item) -> bool| -> u64 {
        r.kinds
            .iter()
            .filter(|k| k.id == kind)
            .flat_map(|k| k.items.iter())
            .filter(|i| f(i))
            .map(|i| i.bytes)
            .sum()
    };
    let trash = item("files", &|i| i.path.ends_with("/.Trash"));
    let unseen = item("other", &|i| i.path.is_empty());
    let mut out: Vec<(&'static str, u64)> = r
        .kinds
        .iter()
        .map(|k| {
            let less = match k.id {
                "files" => trash,
                "other" => unseen,
                _ => 0,
            };
            (k.id, k.bytes.saturating_sub(less))
        })
        .collect();
    out.push(("trash", trash));
    out.push(("unseen", unseen));
    out.retain(|(_, b)| *b > 0);
    out
}

/// Space counted as used that is freed without a clean-up. What Time
/// Machine's local snapshots hold is in purgeable already (macOS counts it
/// there, and deletes them when space is needed); what the scan couldn't
/// see is mostly macOS's own files and metadata, which stay, so it isn't
/// here.
fn comes_back(m: &Measured, b: &ComingBack) -> Vec<ComesBack> {
    let mut out = vec![
        ComesBack {
            what: "purgeable",
            bytes: m.volume.purgeable,
            how: "macOS removes it when space is needed".into(),
            kind: (m.snapshots > 0).then_some("other"),
        },
        ComesBack {
            what: "swap",
            bytes: b.swap,
            how: "removed when the Mac restarts".into(),
            kind: Some("macos"),
        },
    ];
    // The Trash as it is now, not as it was at the scan: emptying it in
    // Finder shows at once.
    let trash = m.home.join(".Trash");
    let trash = if crate::privacy::may_open(&trash) && trash.is_dir() {
        scan::size_of(&trash).unwrap_or(b.trash)
    } else {
        b.trash
    };
    out.push(ComesBack {
        what: "trash",
        bytes: trash,
        how: "freed when the Trash is emptied".into(),
        kind: Some("files"),
    });
    out.retain(|c| c.bytes > 0);
    out
}

impl Report {
    /// With a macOS update that's waiting for the restart that installs it
    /// ([`crate::os_update::staged`]): what it holds comes back then. It's
    /// in macOS, in Preboot, the Update volume and System downloads, which
    /// say so.
    pub fn with_update(mut self, u: Option<&crate::os_update::Staged>) -> Report {
        let Some(u) = u else {
            return self;
        };
        let at = self
            .comes_back
            .iter()
            .position(|c| c.what == "trash")
            .unwrap_or(self.comes_back.len());
        self.comes_back.insert(
            at,
            ComesBack {
                what: "update",
                bytes: u.bytes,
                how: format!("freed once the Mac restarts to install macOS {}", u.version),
                kind: Some("macos"),
            },
        );
        let held = format!(
            "macOS {} is here too, until a restart installs it",
            u.version
        );
        for k in self.kinds.iter_mut().filter(|k| k.id == "macos") {
            for i in &mut k.items {
                if matches!(
                    i.path.as_str(),
                    "/System/Volumes/Preboot"
                        | "/System/Volumes/Update"
                        | "/System/Library/AssetsV2"
                ) {
                    i.note = Some(match &i.note {
                        Some(n) => format!("{n}. {held}"),
                        None => held.clone(),
                    });
                }
            }
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Dates, without a calendar crate.

/// `2026-09-24 05:43:18 +0000` (Spotlight) or `2026-10-16T06:42:53Z`.
pub fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    if s.len() < 19 || !matches!(s.as_bytes()[10], b' ' | b'T') {
        return None;
    }
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let offset = match s[19..].trim() {
        "" | "Z" => 0,
        o if o.len() == 5 && (o.starts_with('+') || o.starts_with('-')) => {
            let h: i64 = o[1..3].parse().ok()?;
            let m: i64 = o[3..5].parse().ok()?;
            let secs = h * 3600 + m * 60;
            if o.starts_with('-') { -secs } else { secs }
        }
        _ => return None,
    };
    Some(days_from_civil(y, mo as u32, d as u32) * DAY + h * 3600 + mi * 60 + sec - offset)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// (year, month 1–12) of a Unix time, in UTC.
pub(crate) fn civil(t: i64) -> (i64, u32) {
    let z = t.div_euclid(DAY) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m)
}

/// "March", or "March 2025" when it isn't the year of `now`.
fn month(t: i64, now: i64) -> String {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let (y, m) = civil(t);
    let name = NAMES[m as usize - 1];
    if y == civil(now).0 {
        name.to_string()
    } else {
        format!("{name} {y}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;
    const MB: u64 = 1_000_000;
    /// 2026-10-21 12:00 UTC.
    const NOW: i64 = 1_792_584_000;
    const HOME: &str = "/Users/me";

    /// A folder with `own` bytes of files directly in it, all private, last
    /// modified a day before [`NOW`].
    fn dir(name: &str, own: u64, children: Vec<Dir>) -> Dir {
        let mut d = Dir {
            name: name.into(),
            allocated: own,
            private: own,
            cold: 0,
            cloned: 0,
            twice: 0,
            mtime: Some(NOW - DAY),
            children,
        };
        // Its folders share nothing with each other.
        for c in &d.children {
            d.allocated += c.allocated;
            d.private += c.private;
            d.cold += c.cold;
            d.cloned += c.cloned;
            d.twice += c.twice;
        }
        d
    }

    /// `own` bytes, all cold (modified five years ago).
    fn cold(name: &str, own: u64, children: Vec<Dir>) -> Dir {
        let mut d = dir(name, 0, children);
        d.allocated += own;
        d.private += own;
        d.cold += own;
        d.mtime = Some(NOW - 5 * 365 * DAY);
        d
    }

    fn leaf(name: &str, own: u64) -> Dir {
        dir(name, own, Vec::new())
    }

    fn measured(tree: Dir, volumes: Vec<OtherVolume>, unseen: u64) -> Measured {
        let data = tree.allocated;
        let others: u64 = volumes.iter().map(|v| v.bytes).sum();
        let used = data + others + unseen;
        Measured {
            version: VERSION,
            measured: NOW,
            took_secs: 1.0,
            home: HOME.into(),
            volume: VolumeSpace {
                total: used + 100 * GB,
                used,
                free: 100 * GB,
                purgeable: 0,
            },
            volumes,
            data_in_use: data + unseen,
            data_real: data,
            snapshots: 0,
            unreadable_dirs: 0,
            tree,
            apps: Vec::new(),
            runtimes: Vec::new(),
            files: Vec::new(),
        }
    }

    fn volume(role: &str, bytes: u64) -> OtherVolume {
        OtherVolume {
            name: role.into(),
            role: Some(role.into()),
            mount: None,
            bytes,
        }
    }

    fn kind<'a>(r: &'a Report, id: &str) -> &'a KindReport {
        r.kinds
            .iter()
            .find(|k| k.id == id)
            .unwrap_or_else(|| panic!("no {id} in {:#?}", r.kinds))
    }

    fn named<'a>(r: &'a Report, id: &str, name: &str) -> &'a Item {
        kind(r, id)
            .items
            .iter()
            .find(|i| i.name == name)
            .unwrap_or_else(|| panic!("no {name} in {:#?}", kind(r, id)))
    }

    /// A Mac with a bit of everything.
    fn a_mac() -> Dir {
        let me = dir(
            "me",
            GB,
            vec![
                dir(
                    "Library",
                    2 * GB,
                    vec![
                        dir("Caches", 300 * MB, vec![leaf("com.spotify.client", 2 * GB)]),
                        dir(
                            "Containers",
                            0,
                            vec![dir(
                                "com.example.chat",
                                3 * GB,
                                vec![dir(
                                    "Data",
                                    0,
                                    vec![dir("Library", 0, vec![leaf("Caches", GB)])],
                                )],
                            )],
                        ),
                        dir(
                            "Application Support",
                            0,
                            vec![dir("Slack", 500 * MB, vec![leaf("Cache", 700 * MB)])],
                        ),
                        dir(
                            "Developer",
                            0,
                            vec![dir("Xcode", 0, vec![leaf("DerivedData", 6 * GB)])],
                        ),
                        dir("CloudStorage", 0, vec![leaf("OneDrive-Personal", 9 * GB)]),
                        leaf("Mail", 4 * GB),
                    ],
                ),
                dir(
                    "devel",
                    5 * GB,
                    vec![dir(
                        "app",
                        GB,
                        vec![
                            dir(
                                "src-tauri",
                                0,
                                vec![dir(
                                    "target",
                                    2 * GB,
                                    vec![
                                        dir("debug", 3 * GB, vec![leaf("incremental", 12 * GB)]),
                                        dir("release", GB, vec![leaf("incremental", 400 * MB)]),
                                    ],
                                )],
                            ),
                            dir(
                                "node_modules",
                                800 * MB,
                                vec![dir("dep", 0, vec![leaf("build", 200 * MB)])],
                            ),
                            dir(
                                ".claude",
                                0,
                                vec![dir(
                                    "worktrees",
                                    0,
                                    vec![dir("fix", 100 * MB, vec![leaf("target", 4 * GB)])],
                                )],
                            ),
                        ],
                    )],
                ),
                dir(
                    "Desktop",
                    GB,
                    vec![
                        cold("Storage", 10 * GB, Vec::new()),
                        cold("Old", 3 * GB, Vec::new()),
                    ],
                ),
                dir(
                    "Pictures",
                    0,
                    vec![leaf("Photos Library.photoslibrary", 20 * GB)],
                ),
                dir(
                    "Virtual Machines.localized",
                    0,
                    vec![leaf("Windows 11.vmwarevm", 25 * GB)],
                ),
                leaf(".Trash", 2 * GB),
                leaf(".gradle", 3 * GB),
            ],
        );
        dir(
            DATA_ROOT,
            10 * MB,
            vec![
                dir(
                    "Applications",
                    100 * MB,
                    vec![
                        dir(
                            "Foo.app",
                            GB,
                            vec![dir("Contents", 0, vec![leaf("Helper.app", 500 * MB)])],
                        ),
                        leaf("Bar.app", 2 * GB),
                    ],
                ),
                dir("Users", 0, vec![me, leaf("Shared", GB)]),
                dir(
                    "System",
                    0,
                    vec![dir(
                        "Library",
                        0,
                        vec![dir(
                            "AssetsV2",
                            3 * GB,
                            vec![dir(
                                "com_apple_MobileAsset_iOSSimulatorRuntime",
                                0,
                                vec![leaf("3e80.asset", 8 * GB)],
                            )],
                        )],
                    )],
                ),
                dir(
                    "private",
                    0,
                    vec![dir(
                        "var",
                        0,
                        vec![leaf("folders", 6 * GB), leaf("db", 4 * GB)],
                    )],
                ),
                dir("opt", 0, vec![leaf("homebrew", 11 * GB)]),
                leaf(".Spotlight-V100", GB),
            ],
        )
    }

    #[test]
    fn every_byte_is_sorted_once_and_the_kinds_add_up() {
        let tree = a_mac();
        let data = tree.allocated;
        let m = measured(
            tree,
            vec![volume("System", 12 * GB), volume("VM", 3 * GB)],
            2 * GB,
        );
        let keep = Keep::parse("~/Desktop/Storage\n", Path::new(HOME));
        let r = report(&m, &keep, &[]);

        let sum: u64 = r.kinds.iter().map(|k| k.bytes).sum();
        assert_eq!(sum, r.volume.used);
        let unseen = named(&r, "other", "System data").bytes;
        assert_eq!(unseen, 2 * GB);
        assert_eq!(
            kind(&r, "macos").bytes,
            12 * GB + 3 * GB + 3 * GB + 4 * GB + GB
        );
        assert_eq!(sum - 15 * GB - unseen, data, "the Data volume, once");
        let order: Vec<u64> = r.kinds.iter().map(|k| k.bytes).collect();
        assert!(order.windows(2).all(|w| w[0] >= w[1]), "{order:?}");

        // One item per app; a helper app inside one is part of it.
        let apps = kind(&r, "apps");
        assert_eq!(apps.bytes, 100 * MB + 3500 * MB);
        let names: Vec<&str> = apps.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Bar", "Foo"]);
        assert_eq!(named(&r, "apps", "Foo").bytes, 1500 * MB);

        // Build output, with a big incremental cache on its own.
        let build = kind(&r, "build");
        assert_eq!(build.bytes, 6400 * MB + 12 * GB + GB + 4 * GB);
        assert_eq!(build.reclaimable, build.bytes);
        let inc = named(&r, "build", "app/src-tauri/target/debug/incremental");
        assert_eq!(inc.bytes, 12 * GB);
        assert_eq!(inc.flags, ["build"]);
        assert!(
            inc.note
                .as_deref()
                .unwrap()
                .starts_with("Rust's incremental")
        );
        // The release one is too small to list apart.
        assert_eq!(named(&r, "build", "app/src-tauri/target").bytes, 6400 * MB);
        assert_eq!(named(&r, "build", "app/node_modules").bytes, GB);
        assert_eq!(
            named(&r, "build", "app/.claude/worktrees/fix/target").bytes,
            4 * GB
        );
        // Nothing here exists, so nothing is offered for deleting.
        assert!(build.items.iter().all(|i| i.action.is_none()));

        // Caches, named after their app.
        let caches = kind(&r, "caches");
        assert_eq!(caches.bytes, 300 * MB + 2 * GB + GB + 700 * MB + 6 * GB);
        assert_eq!(caches.reclaimable, caches.bytes);
        assert_eq!(named(&r, "caches", "com.example.chat").bytes, GB);
        assert_eq!(named(&r, "caches", "Slack").bytes, 700 * MB);
        assert_eq!(named(&r, "caches", "Temporary files").bytes, 6 * GB);
        assert_eq!(
            named(&r, "caches", "com.spotify.client").flags,
            ["cache", "regrows"]
        );
        assert_eq!(named(&r, "appdata", "com.example.chat").bytes, 3 * GB);
        assert_eq!(named(&r, "appdata", "Slack").bytes, 500 * MB);

        let dev = kind(&r, "dev");
        assert_eq!(dev.bytes, 6 * GB + 8 * GB + 11 * GB + 3 * GB);
        assert_eq!(named(&r, "dev", "Xcode DerivedData").flags, ["build"]);
        assert_eq!(named(&r, "dev", "Homebrew").bytes, 11 * GB);
        assert_eq!(named(&r, "dev", "Gradle").bytes, 3 * GB);
        let sim = named(&r, "dev", "iOS Simulator");
        assert_eq!(sim.bytes, 8 * GB);
        assert!(sim.note.as_deref().unwrap().contains("Components"));
        assert_eq!(named(&r, "macos", "System downloads").bytes, 3 * GB);
        // A system folder goes by its path.
        assert_eq!(named(&r, "macos", "/private/var/db").bytes, 4 * GB);
        assert_eq!(named(&r, "macos", "Spotlight index").bytes, GB);

        assert_eq!(named(&r, "cloud", "OneDrive-Personal").bytes, 9 * GB);
        assert_eq!(named(&r, "vms", "Windows 11").bytes, 25 * GB);
        assert_eq!(named(&r, "media", "Photos Library").bytes, 20 * GB);
        assert_eq!(named(&r, "mail", "Mail").bytes, 4 * GB);

        // Your files: kept and cold.
        let desktop = named(&r, "files", "Desktop");
        assert_eq!(desktop.bytes, 14 * GB);
        assert_eq!(desktop.flags, ["cold"], "13 of 14 GB are cold");
        // What's in it, or else since when it's unchanged.
        let note = desktop.note.as_deref().unwrap();
        assert!(
            note.starts_with("Mostly ") || note.starts_with("Unchanged since"),
            "{note}"
        );
        // Only the cold bytes that aren't kept look like waste.
        assert_eq!(kind(&r, "files").reclaimable, 3 * GB);
        // The code folder's source, without its build output.
        assert_eq!(named(&r, "files", "devel").bytes, 6100 * MB);
        assert_eq!(named(&r, "files", "Shared").bytes, GB);

        // What is freed without a clean-up.
        let back: Vec<(&str, u64)> = r.comes_back.iter().map(|c| (c.what, c.bytes)).collect();
        assert_eq!(back, [("swap", 3 * GB), ("trash", 2 * GB)]);
        assert_eq!(
            named(&r, "macos", "Swap").note.as_deref(),
            Some("Removed when the Mac restarts")
        );
    }

    #[test]
    fn a_kept_item_is_flagged_and_never_waste() {
        let m = measured(a_mac(), Vec::new(), 0);
        let keep = Keep::parse(
            "~/devel/app/src-tauri/target\n/Library/Caches/*\n~/Library/Caches/com.spotify.client\n",
            Path::new(HOME),
        );
        let r = report(&m, &keep, &[]);
        let target = named(&r, "build", "app/src-tauri/target");
        assert!(target.flags.contains(&"kept"), "{target:?}");
        // Its incremental caches are inside what's kept.
        let inc = named(&r, "build", "app/src-tauri/target/debug/incremental");
        assert!(inc.flags.contains(&"kept"));
        assert_eq!(kind(&r, "build").reclaimable, GB + 4 * GB);
        assert!(
            named(&r, "caches", "com.spotify.client")
                .flags
                .contains(&"kept")
        );
        let caches = kind(&r, "caches");
        assert_eq!(caches.reclaimable, caches.bytes - 2 * GB);
    }

    #[test]
    fn what_a_snapshot_holds_is_counted_once_in_purgeable() {
        // A Mac with a local snapshot: macOS counts what it holds as
        // purgeable; the scan sees none of it, nor macOS's own files.
        let mut m = measured(a_mac(), vec![volume("VM", 3 * GB)], 25 * GB);
        m.snapshots = 1;
        m.volume.purgeable = 10 * GB;
        m.unreadable_dirs = 294;
        let snap = "com.apple.TimeMachine.2026-10-27-231632.local".to_string();
        // A macOS update's snapshot isn't Time Machine's: never deleted.
        let listed = [snap.clone(), "com.apple.os.update-MSUPrepareUpdate".into()];
        let r = report(&m, &Keep::default(), &listed);
        let back: Vec<(&str, u64)> = r.comes_back.iter().map(|c| (c.what, c.bytes)).collect();
        assert_eq!(
            back,
            [("purgeable", 10 * GB), ("swap", 3 * GB), ("trash", 2 * GB)]
        );
        let unseen = named(&r, "other", "Snapshots and system data");
        assert_eq!(unseen.bytes, 25 * GB);
        assert_eq!(
            unseen.note.as_deref(),
            Some(
                "Up to 10.0 GB in 1 Time Machine snapshot, the rest in folders only macOS can read"
            )
        );
        let detail = unseen.detail.as_deref().unwrap();
        assert!(
            detail.starts_with("A local Time Machine snapshot, from 2026-10-27 23:16, keeps"),
            "{detail}"
        );
        assert!(detail.contains("up to 10.0 GB"), "{detail}");
        assert!(
            detail.contains("The rest, 15.0 GB, is in 294 folders macOS keeps to itself"),
            "{detail}"
        );
        let a = unseen.action.as_ref().unwrap();
        assert_eq!((a.kind, a.label.as_str()), ("snapshots", "Delete Snapshot"));
        assert_eq!(a.paths, [snap]);
        assert!(a.confirm.is_some(), "asked first: it can't be undone");

        // None left: no button, and nothing said about snapshots.
        let r = report(&m, &Keep::default(), &listed[1..]);
        let unseen = named(&r, "other", "System data");
        assert!(unseen.action.is_none());
        assert!(
            unseen
                .detail
                .as_deref()
                .unwrap()
                .starts_with("In 294 folders macOS keeps to itself")
        );
        // Drawn apart from the rest of Other, where the scan couldn't see.
        assert_eq!(r.comes_back[0].kind, Some("other"));
    }

    #[test]
    fn a_folder_named_like_the_start_of_another_says_which_it_is() {
        let lib = tempfile::tempdir().unwrap();
        for d in [
            "Application",
            "Application Support",
            "Application Scripts",
            "Caches",
        ] {
            std::fs::create_dir(lib.path().join(d)).unwrap();
        }
        assert_eq!(
            not_to_be_confused(&lib.path().join("Application")),
            ", the folder named “Application”, not “Application Scripts” or \
             “Application Support”,"
        );
        assert_eq!(not_to_be_confused(&lib.path().join("Caches")), "");
        assert_eq!(
            not_to_be_confused(&lib.path().join("Application Support")),
            ""
        );
    }

    #[test]
    fn a_macos_update_waiting_for_a_restart_comes_back_then() {
        let mut m = measured(
            a_mac(),
            vec![volume("VM", 3 * GB), volume("Preboot", 20 * GB)],
            0,
        );
        m.volumes[1].mount = Some("/System/Volumes/Preboot".into());
        let u = crate::os_update::Staged {
            version: "27.2".into(),
            bytes: 13 * GB,
        };
        let r = report(&m, &Keep::default(), &[]).with_update(Some(&u));
        let back: Vec<_> = r
            .comes_back
            .iter()
            .map(|c| (c.what, c.bytes, c.kind))
            .collect();
        assert_eq!(
            back,
            [
                ("swap", 3 * GB, Some("macos")),
                ("update", 13 * GB, Some("macos")),
                ("trash", 2 * GB, Some("files"))
            ]
        );
        assert!(r.comes_back[1].how.ends_with("install macOS 27.2"));
        assert_eq!(
            named(&r, "macos", "Preboot").note.as_deref(),
            Some(
                "What the Mac starts up from. macOS 27.2 is here too, until a restart \
                 installs it"
            )
        );
        // None waiting: as it was.
        let before = serde_json::to_value(report(&m, &Keep::default(), &[])).unwrap();
        let after =
            serde_json::to_value(report(&m, &Keep::default(), &[]).with_update(None)).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn clones_counted_twice_are_taken_off_where_they_are() {
        // OneDrive keeps its files twice, as clones: 9 GB in its cloud
        // folder and 9 GB in its group container, all shared.
        let shared = |name: &str| Dir {
            name: name.into(),
            allocated: 9 * GB,
            private: 10 * MB,
            cold: 0,
            cloned: 9 * GB - 10 * MB,
            twice: 0,
            mtime: Some(NOW),
            children: Vec::new(),
        };
        let me = dir(
            "me",
            0,
            vec![
                dir(
                    "Library",
                    0,
                    vec![
                        dir("CloudStorage", 0, vec![shared("OneDrive-Personal")]),
                        dir(
                            "Group Containers",
                            0,
                            vec![shared("UBF8T346G9.OneDriveSyncClientSuite")],
                        ),
                    ],
                ),
                leaf("Documents", 5 * GB),
            ],
        );
        let tree = dir(DATA_ROOT, 0, vec![dir("Users", 0, vec![me])]);
        let mut m = measured(tree, Vec::new(), 0);
        // Really used: the clones once.
        m.data_real = 9 * GB + 20 * MB + 5 * GB;
        m.data_in_use = m.data_real + GB;
        m.volume.used = m.data_in_use;
        let r = report(&m, &Keep::default(), &[]);
        let sum: u64 = r.kinds.iter().map(|k| k.bytes).sum();
        assert_eq!(sum, r.volume.used);
        let cloud = kind(&r, "cloud");
        assert_eq!(cloud.bytes, 9 * GB + 20 * MB);
        assert_eq!(cloud.frees, 20 * MB, "deleting either copy frees little");
        assert_eq!(named(&r, "cloud", "OneDrive").bytes, 4510 * MB);
        assert_eq!(
            kind(&r, "files").bytes,
            5 * GB,
            "no clones, nothing taken off"
        );
        assert_eq!(named(&r, "other", "System data").bytes, GB);
    }

    #[test]
    fn a_folder_of_copies_takes_what_its_copies_really_take() {
        // 60 copies of a 4 GB app that Chrome left in a temporary folder,
        // all sharing their blocks: 240 GB as du counts it, 4 GB really.
        let copies = Dir {
            name: "X".into(),
            allocated: 240 * GB,
            private: 0,
            cold: 0,
            cloned: 240 * GB,
            twice: 236 * GB,
            mtime: Some(NOW),
            children: Vec::new(),
        };
        // And 18 GB of OneDrive clones, shared between two folders.
        let shared = |name: &str| Dir {
            name: name.into(),
            allocated: 9 * GB,
            private: 0,
            cold: 0,
            cloned: 9 * GB,
            twice: 0,
            mtime: Some(NOW),
            children: Vec::new(),
        };
        let user = dir("T", 2 * GB, vec![copies]);
        let mut library = dir(
            "Library",
            0,
            vec![
                dir("CloudStorage", 0, vec![shared("OneDrive-Personal")]),
                dir(
                    "Group Containers",
                    0,
                    vec![shared("UBF8T346G9.OneDriveSyncClientSuite")],
                ),
            ],
        );
        library.twice += 9 * GB;
        let tree = dir(
            DATA_ROOT,
            0,
            vec![
                dir(
                    "private",
                    0,
                    vec![dir(
                        "var",
                        0,
                        vec![dir("folders", 0, vec![dir("ab", 0, vec![user])])],
                    )],
                ),
                dir("Users", 0, vec![dir("me", 0, vec![library])]),
            ],
        );
        let mut m = measured(tree, Vec::new(), 0);
        m.data_real = 2 * GB + 4 * GB + 9 * GB;
        m.data_in_use = m.data_real;
        m.volume.used = m.data_in_use;
        let r = report(&m, &Keep::default(), &[]);
        assert_eq!(
            named(&r, "caches", "Temporary files").bytes,
            6 * GB,
            "the copies once, and the rest"
        );
        // What's counted twice across folders still comes off theirs.
        assert_eq!(kind(&r, "cloud").bytes, 9 * GB);
    }

    #[test]
    fn apps_not_opened_for_months_are_unused() {
        let tree = dir(
            DATA_ROOT,
            0,
            vec![dir(
                "Applications",
                0,
                vec![
                    leaf("Old.app", 3 * GB),
                    leaf("Never.app", 2 * GB),
                    leaf("Daily.app", GB),
                    leaf("New.app", GB),
                    leaf("Kept.app", GB),
                ],
            )],
        );
        let mut m = measured(tree, Vec::new(), 0);
        let at = |name: &str, used: Option<i64>, added: Option<i64>| AppDates {
            path: format!("/Applications/{name}.app"),
            last_used: used,
            added,
        };
        m.apps = vec![
            at("Old", Some(1_772_366_400), Some(NOW - 900 * DAY)), // 2026-03-01
            at("Never", None, Some(NOW - 100 * DAY)),
            at("Daily", Some(NOW - DAY), Some(NOW - 900 * DAY)),
            at("New", None, Some(NOW - 10 * DAY)),
            at("Kept", Some(NOW - 400 * DAY), None),
        ];
        let keep = Keep::parse("/Applications/Kept.app\n", Path::new(HOME));
        let r = report(&m, &keep, &[]);
        let old = named(&r, "apps", "Old");
        assert_eq!(old.flags, ["unused"]);
        assert_eq!(old.note.as_deref(), Some("Not opened since March"));
        assert_eq!(old.last_used, Some(1_772_366_400));
        assert_eq!(
            named(&r, "apps", "Never").note.as_deref(),
            Some("Never opened")
        );
        assert!(named(&r, "apps", "Daily").flags.is_empty());
        assert!(named(&r, "apps", "New").flags.is_empty());
        let kept = named(&r, "apps", "Kept");
        assert_eq!(kept.flags, ["unused", "kept"]);
        assert_eq!(
            kept.note.as_deref(),
            Some("Not opened since September 2025")
        );
        assert_eq!(kind(&r, "apps").reclaimable, 5 * GB);
        assert!(kind(&r, "apps").items.iter().all(|i| i.action.is_none()));
    }

    #[test]
    fn spotlight_and_simctl_dates_are_read() {
        let out = b"2026-09-24 05:43:18 +0000\x00NONE\x002024-02-29 12:00:00 +0200";
        assert_eq!(
            parse_mdls(out),
            [Some(1_790_228_598), None, Some(1_709_200_800)]
        );
        assert_eq!(parse_mdls(b"NONE\0"), [None]);
        assert!(parse_mdls(b"").is_empty());
        assert_eq!(parse_time("2026-10-16T06:42:53Z"), Some(1_792_132_973));
        assert_eq!(parse_time("1969-12-31 00:00:00 +0000"), Some(-86_400));
        assert_eq!(parse_time("(null)"), None);
        assert_eq!(parse_time("2026-13-01 00:00:00 +0000"), None);
        assert_eq!(civil(1_790_228_598), (2026, 9));
        assert_eq!(civil(-86_400), (1969, 12));
        assert_eq!(month(1_709_200_800, NOW), "February 2024");

        let json = br#"{
          "2BFB81AB" : {
            "lastUsedAt" : "2026-10-16T06:42:53Z",
            "path" : "/System/Library/AssetsV2/com_apple_MobileAsset_iOSSimulatorRuntime/3e80.asset/AssetData/Restore/043.dmg",
            "runtimeIdentifier" : "com.apple.CoreSimulator.SimRuntime.iOS-27-0",
            "version" : "27.0"
          }
        }"#;
        let rt = parse_runtimes(json);
        assert_eq!(
            rt,
            [Runtime {
                path:
                    "/System/Library/AssetsV2/com_apple_MobileAsset_iOSSimulatorRuntime/3e80.asset"
                        .into(),
                name: "iOS 27.0 Simulator".into(),
                last_used: Some(1_792_132_973),
            }]
        );
        assert!(parse_runtimes(b"not json").is_empty());

        // Named after the runtime it holds, when Xcode says.
        let mut m = measured(a_mac(), Vec::new(), 0);
        m.runtimes = rt;
        let r = report(&m, &Keep::default(), &[]);
        let sim = named(&r, "dev", "iOS 27.0 Simulator");
        assert_eq!(sim.last_used, Some(1_792_132_973));
    }

    #[test]
    fn a_clean_up_is_measured_again_where_it_was_and_above() {
        // A real folder with two big-enough parts, measured as the whole-disk
        // scan would, then set in a tree from the root down to it.
        let tmp = tempfile::tempdir().unwrap();
        let top = tmp.path().canonicalize().unwrap().join("project");
        for part in ["keep", "goes"] {
            std::fs::create_dir_all(top.join(part)).unwrap();
            std::fs::write(top.join(part).join("data"), vec![1u8; 300_000]).unwrap();
        }
        let opts = scan::Options {
            min_node: 1,
            ..TREE
        };
        let mut inner = Dir::from_node(&scan::scan(&top, &opts).unwrap().root, false);
        inner.name = "project".into();
        let before = inner.allocated;
        let names: Vec<String> = top
            .strip_prefix("/")
            .unwrap()
            .iter()
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        let mut tree = inner;
        for n in names[..names.len() - 1].iter().rev() {
            tree = Dir {
                name: n.clone(),
                allocated: tree.allocated + 5 * GB,
                private: tree.private + 5 * GB,
                children: vec![tree],
                ..Dir::default()
            };
        }
        let root = Dir {
            name: DATA_ROOT.into(),
            allocated: tree.allocated,
            private: tree.private,
            children: vec![tree],
            ..Dir::default()
        };
        let mut m = measured(root, Vec::new(), 0);
        let root_before = m.tree.allocated;

        // "goes" is deleted, then measured again: it's gone from the tree,
        // and what it held is off every folder above it.
        let goes = top.join("goes");
        let gone = node_mut(&mut m.tree, &[names.clone(), vec!["goes".into()]].concat())
            .unwrap()
            .allocated;
        assert!(gone > 0 && gone < before);
        std::fs::remove_dir_all(&goes).unwrap();
        update(&mut m, &[goes]).unwrap();
        let project = node_mut(&mut m.tree, &names).unwrap();
        assert!(project.children.iter().all(|c| c.name != "goes"));
        assert_eq!(project.allocated, before - gone);
        assert_eq!(m.tree.allocated, root_before - gone);
        // Free space is what the disk says now.
        assert_eq!(m.volume.used + m.volume.free, m.volume.total);
    }

    #[test]
    fn a_measurement_is_kept_and_read_back_whole() {
        let mut m = measured(a_mac(), vec![volume("VM", GB)], GB);
        m.apps = vec![AppDates {
            path: "/Applications/Foo.app".into(),
            last_used: Some(NOW),
            added: None,
        }];
        m.files = vec![big_file("/Users/me/Movies/old.mov", 2 * GB, 400, 500)];
        let text = serde_json::to_string(&m).unwrap();
        let back: Measured = serde_json::from_str(&text).unwrap();
        assert_eq!(back.tree, m.tree);
        assert_eq!(back.apps, m.apps);
        assert_eq!(back.files, m.files);
        assert_eq!(
            serde_json::to_value(report(&back, &Keep::default(), &[])).unwrap(),
            serde_json::to_value(report(&m, &Keep::default(), &[])).unwrap()
        );
    }

    /// A file of `bytes`, all private, last read and changed that many days
    /// before [`NOW`].
    fn big_file(path: &str, bytes: u64, read: i64, changed: i64) -> BigFile {
        BigFile {
            path: path.into(),
            allocated: bytes,
            private: bytes,
            accessed: NOW - read * DAY,
            modified: NOW - changed * DAY,
        }
    }

    #[test]
    fn only_your_own_big_files_are_kept_biggest_first() {
        let found = |rel: &str, bytes: u64| scan::BigFile {
            path: Path::new(DATA_ROOT).join(rel),
            allocated: bytes,
            private: bytes / 2,
            atime: 1,
            mtime: 2,
        };
        let files = own_files(
            &[
                found("Users/me/Movies/trip.mov", 3 * GB),
                found("Users/me/Desktop/old.dmg", 5 * GB),
                found("Users/Shared/installer.pkg", GB),
                found("Users/me/Library/Containers/x/Data/big.db", 9 * GB),
                found("Users/me/.ollama/models/blob", 8 * GB),
                found("Users/me/devel/app/target/debug/app", 7 * GB),
                found("Users/me/devel/app/.git/objects/pack/p.pack", 6 * GB),
                found(
                    "Users/me/Pictures/Photos Library.photoslibrary/originals/a.mov",
                    4 * GB,
                ),
                found("Users/me/VMs/Windows.vmwarevm/disk.vmdk", 4 * GB),
                found("Users/other/Movies/theirs.mov", 4 * GB),
                found("Applications/Big.app/Contents/Resources/x", 4 * GB),
                found("private/var/vm/sleepimage", 4 * GB),
            ],
            Path::new(HOME),
        );
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "/Users/me/Desktop/old.dmg",
                "/Users/me/Movies/trip.mov",
                "/Users/Shared/installer.pkg"
            ]
        );
        assert_eq!(
            files[0],
            BigFile {
                path: "/Users/me/Desktop/old.dmg".into(),
                allocated: 5 * GB,
                private: 5 * GB / 2,
                accessed: 1,
                modified: 2,
            }
        );
        let many: Vec<scan::BigFile> = (0..FILES as u64 + 10)
            .map(|i| found(&format!("Users/me/Movies/{i}.mov"), GB + i))
            .collect();
        let kept = own_files(&many, Path::new(HOME));
        assert_eq!(kept.len(), FILES);
        assert_eq!(kept[0].allocated, GB + FILES as u64 + 9, "the biggest");
    }

    #[test]
    fn big_files_unused_for_months_are_old_files() {
        // Small files stand in for big ones: what the scan measured is
        // what's listed, as long as they are still there.
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let mut m = measured(a_mac(), Vec::new(), 0);
        m.home = h.to_path_buf();
        let file = |rel: &str, bytes: u64, read: i64, changed: i64| {
            let p = h.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "x").unwrap();
            big_file(&p.to_string_lossy(), bytes, read, changed)
        };
        m.files = vec![
            file("Movies/recent.mov", 9 * GB, 5, 400),
            file("Movies/changed.mov", 8 * GB, 400, 30),
            file("Desktop/Storage/reel.mov", 3 * GB, 200, 900),
            file("Downloads/old.dmg", 2 * GB, 100, 100),
            file("Documents/huge.zip", 6 * GB, 365, 2000),
            file("Movies/trashed.mov", 7 * GB, 900, 900),
        ];
        std::fs::remove_file(h.join("Movies/trashed.mov")).unwrap();
        let keep = Keep::parse("~/Desktop/Storage\n", h);
        let r = report(&m, &keep, &[]);
        let old: Vec<(&str, bool)> = r
            .old_files
            .iter()
            .map(|f| (f.name.as_str(), f.kept))
            .collect();
        assert_eq!(
            old,
            [("huge.zip", false), ("reel.mov", true), ("old.dmg", false)],
            "biggest first, not the recent ones nor the one gone since"
        );
        let zip = &r.old_files[0];
        assert_eq!((zip.bytes, zip.frees), (6 * GB, 6 * GB));
        assert_eq!(zip.accessed, NOW - 365 * DAY);
        assert_eq!(zip.modified, NOW - 2000 * DAY);
        // Too small now, and just made: none is offered for the Trash.
        assert!(r.old_files.iter().all(|f| f.action.is_none()));
    }

    #[test]
    fn an_old_file_that_is_there_is_offered_for_the_trash() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let file = h.join("Movies/old.mov");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        // Written, not sparse: its allocated size is what counts.
        std::fs::write(&file, vec![7u8; clean::BIG_FILE as usize]).unwrap();
        let now = crate::watch::unix_now();
        let at = |t: i64| std::time::UNIX_EPOCH + Duration::from_secs(t as u64);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_accessed(at(now - 200 * DAY))
                    .set_modified(at(now - 800 * DAY)),
            )
            .unwrap();
        let mut m = measured(a_mac(), Vec::new(), 0);
        m.home = h.to_path_buf();
        m.measured = now;
        let path = file.to_string_lossy().into_owned();
        m.files = vec![BigFile {
            path: path.clone(),
            allocated: clean::BIG_FILE + 4096,
            private: clean::BIG_FILE + 4096,
            accessed: now - 200 * DAY,
            modified: now - 800 * DAY,
        }];
        let r = report(&m, &Keep::default(), &[]);
        let action = r.old_files[0].action.as_ref().expect("a button");
        assert_eq!(
            (action.kind, action.label.as_str()),
            ("trash", "Move to Trash")
        );
        assert_eq!(action.paths, [path]);
        assert_eq!(action.confirm, None, "the Trash gives it back");
        // Kept: listed, no button.
        let keep = Keep::parse(&format!("{}\n", h.join("Movies").display()), h);
        let r = report(&m, &keep, &[]);
        assert!(r.old_files[0].kept && r.old_files[0].action.is_none());
    }

    #[test]
    fn volumes_are_found_mounted_from_themselves_or_a_snapshot() {
        let mounts = [
            ("/dev/disk3s1s1", "/"),
            ("/dev/disk3s5", "/System/Volumes/Data"),
            ("/dev/disk3s6", "/System/Volumes/VM"),
            ("/dev/disk3s10", "/Volumes/Other"),
        ]
        .map(|(d, m)| (d.to_string(), m.to_string()));
        assert_eq!(mount_of(&mounts, "disk3s1").as_deref(), Some("/"));
        assert_eq!(
            mount_of(&mounts, "disk3s6").as_deref(),
            Some("/System/Volumes/VM")
        );
        assert_eq!(mount_of(&mounts, "disk3s3"), None);
    }

    /// The fixture the app decodes (app/Tests/space.json), held to the real
    /// serialization: the same keys at every level.
    #[test]
    fn the_apps_space_fixture_has_the_shape_space_prints() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../app/Tests/space.json")).unwrap();
        let real = serde_json::to_value(Report {
            measured: 0,
            took_secs: 0.0,
            volume: VolumeSpace::default(),
            comes_back: vec![ComesBack {
                what: "swap",
                bytes: 1,
                how: String::new(),
                kind: Some("macos"),
            }],
            kinds: vec![KindReport {
                id: "build",
                title: "",
                bytes: 1,
                frees: 1,
                reclaimable: 1,
                items: vec![Item {
                    path: String::new(),
                    name: String::new(),
                    bytes: 1,
                    frees: 1,
                    age: None,
                    last_used: None,
                    flags: vec!["build"],
                    note: None,
                    detail: None,
                    action: Some(Action {
                        kind: "delete",
                        label: String::new(),
                        paths: Vec::new(),
                        confirm: None,
                        done: None,
                    }),
                    about: Some(About {
                        path: String::new(),
                        bytes: 1,
                        title: "",
                        what: "",
                        how: "",
                        open: crate::handlers::HANDLERS.iter().find_map(|h| h.open),
                        action: None,
                    }),
                }],
            }],
            old_files: vec![OldFile {
                path: String::new(),
                name: String::new(),
                bytes: 1,
                frees: 1,
                accessed: 0,
                modified: 0,
                kept: false,
                action: Some(Action {
                    kind: "trash",
                    label: String::new(),
                    paths: Vec::new(),
                    confirm: None,
                    done: None,
                }),
            }],
        })
        .unwrap();
        let keys = |v: &serde_json::Value| {
            v.as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(keys(&fixture), keys(&real));
        assert_eq!(keys(&fixture["volume"]), keys(&real["volume"]));
        let back = fixture["comes_back"].as_array().unwrap();
        assert!(!back.is_empty());
        for c in back {
            assert_eq!(keys(c), keys(&real["comes_back"][0]));
        }
        let real_kind = &real["kinds"][0];
        let real_item = &real_kind["items"][0];
        let mut actions = 0;
        let mut abouts = 0;
        for k in fixture["kinds"].as_array().unwrap() {
            assert_eq!(keys(k), keys(real_kind));
            assert!(Kind::ALL.iter().any(|kind| kind.id() == k["id"]), "{k}");
            for i in k["items"].as_array().unwrap() {
                assert_eq!(keys(i), keys(real_item));
                if !i["action"].is_null() {
                    assert_eq!(keys(&i["action"]), keys(&real_item["action"]));
                    actions += 1;
                }
                if !i["about"].is_null() {
                    assert_eq!(keys(&i["about"]), keys(&real_item["about"]));
                    assert_eq!(keys(&i["about"]["open"]), keys(&real_item["about"]["open"]));
                    abouts += 1;
                }
            }
        }
        assert!(actions > 0, "the fixture shows an action");
        assert!(abouts > 0, "the fixture shows what an app keeps");
        let real_old = &real["old_files"][0];
        let old = fixture["old_files"].as_array().unwrap();
        for f in old {
            assert_eq!(keys(f), keys(real_old));
            if !f["action"].is_null() {
                assert_eq!(keys(&f["action"]), keys(&real_old["action"]));
                assert_eq!(f["action"]["kind"], "trash");
            }
        }
        assert!(old.iter().any(|f| !f["action"].is_null()), "a Trash button");
        assert!(old.iter().any(|f| f["kept"] == true), "a kept file");
        // It adds up, as the real thing does.
        let used = fixture["volume"]["used"].as_u64().unwrap();
        let kinds: u64 = fixture["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k["bytes"].as_u64().unwrap())
            .sum();
        assert_eq!(kinds, used);
    }
}
