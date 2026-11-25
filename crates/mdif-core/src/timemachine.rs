//! Time Machine: what goes into the backups, what the local snapshots hold
//! and why, and what helps.
//!
//! - Backups: Time Machine's settings
//!   (`/Library/Preferences/com.apple.TimeMachine.plist`, readable without
//!   an administrator) say where it backs up, how often, which backups
//!   finished and which tries didn't, and what it leaves out.
//! - Local snapshots: before each try Time Machine takes a snapshot of the
//!   whole Data volume, excluded folders too: an APFS snapshot can't leave
//!   anything out. It keeps every block that was there, so what's deleted
//!   or rewritten since frees nothing until the snapshot goes. macOS deletes
//!   them after a day, but keeps the one the next backup starts from (the
//!   reference) until a backup finishes: when tries keep failing, that one
//!   holds everything since the last good backup ([`Backlog`]).
//! - What goes in: what changed since the last backup (the watcher's
//!   history), and the folders a build or an install makes again
//!   ([`rebuildable`]) or their tool downloads again ([`download_cache`])
//!   still backed up. Keeping those out (`exclude`) means less to send over
//!   the network and less room on the backup disk; not less in the
//!   snapshots.
//!
//! Whether something is backed up is `tmutil isexcluded`'s answer, which
//! knows the settings' list and macOS's own exclusions (caches, the Trash).

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::Serialize;

use crate::clean::{self, Action};
use crate::keep::Keep;
use crate::owners::ARTIFACTS;
use crate::store::Store;
use crate::{privacy, scan, space, worktrees};

/// Time Machine's settings.
pub const PREFS: &str = "/Library/Preferences/com.apple.TimeMachine.plist";

const HOUR: i64 = 3600;
const DAY: i64 = 86_400;
/// The tries shown: those of the last two weeks, at most this many.
const TRIES: usize = 12;
const TRIES_SPAN: i64 = 14 * DAY;
/// What changed since the last backup: folders that changed this much.
const CHANGE_MIN: i64 = 100_000_000;
/// A tool's cache smaller than this isn't worth keeping out.
pub const CACHE_MIN: u64 = 10_000_000;
/// The mark on a folder you let back into the backups, so the tidy-up
/// doesn't keep it out again; it goes where the folder goes.
pub const LET_IN: &str = "io.github.lgibelli.mydiskisfull.backed-up";

/// The same, on the folder it's in, by name: a build tool deletes and
/// makes again `target` (`cargo clean`), and the mark on it goes with it.
fn let_in_child(name: &std::ffi::OsStr) -> Option<String> {
    let mark = format!("{LET_IN}.{}", name.to_str()?);
    // An extended attribute's name has 127 bytes at most.
    (mark.len() <= 127).then_some(mark)
}

/// Whether you let `dir` back into the backups, by its mark or the one its
/// folder keeps for it. When that can't be read, it counts as yes: nothing
/// is kept out on a guess.
pub fn let_in(dir: &Path) -> bool {
    if crate::os::fs::has_mark(dir, LET_IN).unwrap_or(true) {
        return true;
    }
    match (dir.parent(), dir.file_name().and_then(let_in_child)) {
        (Some(parent), Some(mark)) => crate::os::fs::has_mark(parent, &mark).unwrap_or(true),
        _ => false,
    }
}

/// System Settings' own list of folders kept out (`SkipPaths`), as paths:
/// it writes the ones you add as `~/Downloads`.
fn settings_skips(skip: &[String], home: &Path) -> Vec<PathBuf> {
    skip.iter()
        .map(|p| match p.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                PathBuf::from(format!("{}{rest}", home.to_string_lossy()))
            }
            _ => PathBuf::from(p),
        })
        .collect()
}

const CHANGES: usize = 8;

/// What Time Machine's settings say.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prefs {
    pub auto: bool,
    /// Seconds between automatic backups (hourly when the settings don't
    /// say).
    pub interval: i64,
    /// What they leave out, as written there.
    pub skip: Vec<String>,
    pub destinations: Vec<Destination>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Destination {
    /// The backup disk's name ("Backups").
    pub name: String,
    /// The server, for a network one ("NAS"): never the account.
    pub server: Option<String>,
    pub quota: Option<u64>,
    /// On the backup disk.
    pub used: Option<u64>,
    pub available: Option<u64>,
    /// When each backup that's still there finished (Unix seconds).
    #[serde(skip)]
    pub finished: Vec<i64>,
    /// When the latest tries started.
    #[serde(skip)]
    pub attempts: Vec<i64>,
    /// The local snapshot the next backup starts from.
    #[serde(skip)]
    pub reference: Option<i64>,
}

/// Reads Time Machine's settings; none when it isn't set up, an error
/// when they are there but can't be read (without Full Disk Access).
pub fn prefs() -> std::io::Result<Option<Prefs>> {
    match std::fs::read(PREFS) {
        Ok(bytes) => Ok(parse_prefs(&bytes).ok()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn parse_prefs(bytes: &[u8]) -> Result<Prefs> {
    let v = plist::Value::from_reader(std::io::Cursor::new(bytes))
        .context("reading Time Machine's settings")?;
    let d = v.as_dictionary().context("not a dictionary")?;
    let int = |d: &plist::Dictionary, k: &str| {
        d.get(k).and_then(|v| {
            v.as_signed_integer()
                .or_else(|| v.as_unsigned_integer().map(|u| u as i64))
                .or_else(|| v.as_boolean().map(i64::from))
        })
    };
    let dates = |d: &plist::Dictionary, k: &str| -> Vec<i64> {
        let mut out: Vec<i64> = d
            .get(k)
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_date())
            .filter_map(unix)
            .collect();
        out.sort();
        out
    };
    let destinations = d
        .get("Destinations")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_dictionary())
        .map(|dest| Destination {
            name: dest
                .get("LastKnownVolumeName")
                .and_then(|v| v.as_string())
                .unwrap_or("Backup disk")
                .to_string(),
            server: dest
                .get("NetworkURL")
                .and_then(|v| v.as_string())
                .and_then(server),
            quota: int(dest, "QuotaGB")
                .filter(|&q| q > 0)
                .map(|q| q as u64 * 1_000_000_000),
            used: int(dest, "BytesUsed").map(|b| b.max(0) as u64),
            available: int(dest, "BytesAvailable").map(|b| b.max(0) as u64),
            finished: dates(dest, "SnapshotDates"),
            attempts: dates(dest, "AttemptDates"),
            reference: dest
                .get("ReferenceLocalSnapshotDate")
                .and_then(|v| v.as_date())
                .and_then(unix),
        })
        .collect();
    Ok(Prefs {
        auto: int(d, "AutoBackup").is_some_and(|a| a != 0),
        interval: int(d, "AutoBackupInterval")
            .filter(|&i| i > 0)
            .unwrap_or(HOUR),
        skip: d
            .get("SkipPaths")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_string())
            .map(String::from)
            .collect(),
        destinations,
    })
}

fn unix(d: plist::Date) -> Option<i64> {
    let t: std::time::SystemTime = d.into();
    t.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// The server in a backup disk's URL, without the account:
/// `smb://GUEST:@NAS._smb._tcp.local./timemachine` is "NAS".
pub fn server(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split('/').next()?;
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = match host.split_once("._") {
        Some((name, _)) => name,
        None => host
            .trim_end_matches('.')
            .trim_end_matches(".local")
            .split(':')
            .next()
            .unwrap_or(host),
    };
    (!host.is_empty()).then(|| host.to_string())
}

/// A try at a backup: when it started, and when it finished, if it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Try {
    pub started: i64,
    pub finished: Option<i64>,
}

/// The tries, newest first: each try finished if a backup finished after
/// it started and before the next one did. A backup with no try recorded
/// is a try of its own.
pub fn tries(finished: &[i64], attempts: &[i64]) -> Vec<Try> {
    let mut out: Vec<Try> = Vec::new();
    let mut matched = vec![false; finished.len()];
    for (i, &a) in attempts.iter().enumerate() {
        let next = attempts.get(i + 1).copied().unwrap_or(i64::MAX);
        let done = finished.iter().position(|&f| f >= a && f < next);
        if let Some(j) = done {
            matched[j] = true;
        }
        out.push(Try {
            started: a,
            finished: done.map(|j| finished[j]),
        });
    }
    for (j, &f) in finished.iter().enumerate() {
        if !matched[j] {
            out.push(Try {
                started: f,
                finished: Some(f),
            });
        }
    }
    out.sort_by_key(|t| std::cmp::Reverse(t.started));
    out
}

/// Time Machine's backups failing while its snapshots hold what changed
/// since the last good one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Backlog {
    /// When the last backup finished; none, never.
    pub since: Option<i64>,
    /// Tries since then that didn't finish.
    pub failed: usize,
    /// What macOS counts as purgeable, which is at most what the snapshots
    /// hold.
    pub held: u64,
    /// The backup disk it is, when there are several ([`worst_backlog`]).
    pub to: Option<String>,
}

/// Backups are failing when two tries in a row since the last good backup
/// didn't finish; overdue when none finished in twice the time between
/// them (a day at least: a Mac that's off doesn't back up), unless one is
/// under way, catching up. One under way doesn't count as failed. Either
/// way the snapshot holds what changed since; `failed` says which it is.
pub fn backlog(
    tries: &[Try],
    interval: i64,
    running: bool,
    held: u64,
    now: i64,
) -> Option<Backlog> {
    let since = tries.iter().find_map(|t| t.finished);
    let failed = tries
        .iter()
        .take_while(|t| t.finished.is_none())
        .skip(usize::from(running))
        .count();
    let overdue = !running && since.is_some_and(|s| now - s > (2 * interval).max(DAY));
    (failed >= 2 || overdue || (since.is_none() && failed > 0)).then_some(Backlog {
        since,
        failed,
        held,
        to: None,
    })
}

/// Each backup disk's [`backlog`], judged by its own tries (a USB disk
/// backing up every hour says nothing of a NAS out of reach for two
/// weeks), and of those behind, the one furthest behind, named when there
/// are several: it holds the most in its snapshot.
pub fn worst_backlog(
    destinations: &[Destination],
    interval: i64,
    running: bool,
    held: u64,
    now: i64,
) -> Option<Backlog> {
    let several = destinations.len() > 1;
    destinations
        .iter()
        .filter_map(|d| {
            let (mut finished, mut attempts) = (d.finished.clone(), d.attempts.clone());
            finished.sort();
            attempts.sort();
            let mut b = backlog(&tries(&finished, &attempts), interval, running, held, now)?;
            b.to = several.then(|| d.name.clone());
            Some(b)
        })
        // Never backed up is furthest behind.
        .min_by_key(|b| (b.since.unwrap_or(i64::MIN), std::cmp::Reverse(b.failed)))
}

/// A backup under way.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Running {
    /// How far it is, 0 to 1, when it says.
    pub done: Option<f64>,
    /// "Copying", "ThinningPostBackup"...
    pub phase: Option<String>,
}

/// `tmutil status`: a backup under way, or none.
pub fn running() -> Option<Running> {
    let out = Command::new("tmutil")
        .arg("status")
        .stderr(Stdio::null())
        .output()
        .ok()?;
    parse_status(&String::from_utf8_lossy(&out.stdout))
}

/// `tmutil status` prints an old-style property list:
/// `Running = 1; Percent = "0.42"; BackupPhase = Copying;`.
pub fn parse_status(out: &str) -> Option<Running> {
    let field = |k: &str| {
        out.lines().find_map(|l| {
            let (key, v) = l.trim().split_once(" = ")?;
            (key == k).then(|| v.trim_end_matches(';').trim_matches('"').to_string())
        })
    };
    if field("Running").as_deref() != Some("1") {
        return None;
    }
    Some(Running {
        done: field("Percent")
            .and_then(|p| p.parse::<f64>().ok())
            .filter(|p| (0.0..=1.0).contains(p)),
        phase: field("BackupPhase"),
    })
}

/// A local Time Machine snapshot.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Snapshot {
    /// As `tmutil listlocalsnapshots` gives it.
    pub name: String,
    pub taken: i64,
    /// The one the next backup starts from: kept until a backup finishes.
    pub reference: bool,
}

/// The moment a local wall-clock time names (`wall`: its fields read as if
/// UTC). The offset is the one at that moment, not at `wall` taken as UTC:
/// in the hours around a change of summer time those differ.
fn from_wall(wall: i64) -> i64 {
    let guess = wall - crate::os::sys::utc_offset(wall);
    wall - crate::os::sys::utc_offset(guess)
}

/// Time Machine's own among `names`, oldest first, each backup disk's
/// reference marked (within a minute of one of `references`).
pub fn snapshots(names: &[String], references: &[i64]) -> Vec<Snapshot> {
    let mut out: Vec<Snapshot> = names
        .iter()
        .filter_map(|n| {
            let d = clean::snapshot_date(Path::new(n))?;
            // Named in local time: 2026-10-29-093349.
            let text = format!(
                "{} {}:{}:{} +0000",
                d.get(..10)?,
                d.get(11..13)?,
                d.get(13..15)?,
                d.get(15..17)?
            );
            let taken = from_wall(space::parse_time(&text)?);
            Some(Snapshot {
                name: n.clone(),
                taken,
                reference: references.iter().any(|r| (r - taken).abs() <= 60),
            })
        })
        .collect();
    out.sort_by_key(|s| s.taken);
    out
}

/// A folder that changed since the last backup.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Change {
    pub path: String,
    /// What it's called, when that's known ([`knowledge`](crate::knowledge)):
    /// "Photo analysis (macOS)" for `com.apple.mediaanalysisd`.
    pub name: Option<String>,
    /// What's in it, why it grows and what helps, when that's known.
    pub known: Option<crate::knowledge::Known>,
    /// Nothing is said about it: its name as it may be sent to ask
    /// ([`knowledge::generic`](crate::knowledge::generic)).
    pub unexplained: Option<String>,
    /// Grew (goes into the next backup) or shrank (what went stays in the
    /// snapshots until then).
    pub bytes: i64,
    pub backed_up: Option<bool>,
    /// Folders made again inside it that Time Machine skips: a new
    /// worktree is mostly its node_modules, so less goes into the backup
    /// than it grew.
    pub kept_out: usize,
}

/// A folder a build, an install or its tool makes again.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Folder {
    pub path: String,
    /// `build` (output), `dependencies` (downloaded packages) or `cache` (a
    /// tool's downloads).
    pub what: &'static str,
    pub bytes: u64,
    pub backed_up: bool,
    /// The git checkout it's in (a worktree's own folder), and the project
    /// that checkout belongs to: they differ for a worktree. A tool's cache
    /// is in the tool's folder ([`tool_folder`]), both.
    pub checkout: String,
    pub project: String,
}

/// Where `dir` is: the nearest folder above it with a `.git` (its
/// checkout), and the nearest with a `.git` folder (its project). A
/// worktree's or a submodule's `.git` is a file, so an agent's worktree
/// inside its repository (`.claude/worktrees/x`) is that repository's; one
/// elsewhere is its own project. Without either, the folder above `dir`.
pub fn project(dir: &Path, home: &Path) -> (PathBuf, PathBuf) {
    let mut checkout = None;
    for p in dir.ancestors().skip(1) {
        if p == home || !p.starts_with(home) {
            break;
        }
        match p.join(".git").symlink_metadata() {
            Ok(m) if m.is_dir() => {
                return (checkout.unwrap_or_else(|| p.to_path_buf()), p.to_path_buf());
            }
            Ok(_) if checkout.is_none() => checkout = Some(p.to_path_buf()),
            _ => {}
        }
    }
    let checkout = checkout.unwrap_or_else(|| dir.parent().unwrap_or(dir).to_path_buf());
    (checkout.clone(), checkout)
}

/// Whether `dir` is a folder its project makes again, and which kind:
/// `dependencies` ([`clean::dependencies`], and a `.venv` or `Pods` git
/// ignores), or `build` ([`clean::build_output`], and any other folder named
/// as build output that git ignores and tracks nothing in: Xcode's `build`,
/// a `dist`).
pub fn rebuildable(dir: &Path) -> Option<&'static str> {
    let name = dir.file_name()?.to_str()?;
    if !ARTIFACTS.contains(&name) {
        return None;
    }
    if clean::dependencies(dir) {
        return Some("dependencies");
    }
    if clean::build_output(dir) {
        return Some("build");
    }
    let ignored = privacy::may_open(dir)
        && dir.is_dir()
        && worktrees::ignores(dir) == Some(true)
        && worktrees::tracks_files_in(dir) == Some(false);
    ignored.then_some(match name {
        "node_modules" | ".venv" | "Pods" => "dependencies",
        _ => "build",
    })
}

/// The folders in the home folder that are macOS's or yours, not a tool's:
/// no tool's cache is looked for in them, nor in the code folders
/// ([`CODE_ROOTS`](crate::owners::CODE_ROOTS), whatever the case), whose
/// projects have build folders instead.
const NOT_TOOLS: &[&str] = &[
    "Applications",
    "Desktop",
    "Documents",
    "Downloads",
    "Library",
    "Movies",
    "Music",
    "Pictures",
    "Public",
    "Sites",
    ".Trash",
];

/// Tools that keep their folder in the home folder under a name of their
/// own, not a hidden one: Go's (`~/go/pkg/mod/cache`), Flutter's SDK and
/// conda's.
const TOOL_HOMES: &[&str] = &["go", "flutter", "miniconda3", "anaconda3", "miniforge3"];

/// The tool's folder in the home folder that `dir` is in (`~/.bun` for
/// `~/.bun/install/cache`), or `dir` itself: a hidden folder (where tools
/// keep theirs), or one of [`TOOL_HOMES`]. None for what's in macOS's
/// folders ([`NOT_TOOLS`]), the code folders, or a folder of yours
/// (`~/Research/scraper/cache` is yours, however it's named).
pub fn tool_folder(dir: &Path, home: &Path) -> Option<PathBuf> {
    let Some(Component::Normal(name)) = dir.strip_prefix(home).ok()?.components().next() else {
        return None;
    };
    let name = name.to_str()?;
    let tools = name.starts_with('.') || TOOL_HOMES.contains(&name);
    (tools && !NOT_TOOLS.contains(&name) && !crate::owners::is_code_root(name))
        .then(|| home.join(name))
}

/// Whether `dir` has a `CACHEDIR.TAG` as the standard has it: a file that
/// starts with its signature, not one that only has the name.
fn tagged_cache(dir: &Path) -> bool {
    use std::io::Read;
    const SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";
    let mut head = [0u8; SIGNATURE.len()];
    std::fs::File::open(dir.join("CACHEDIR.TAG"))
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| head == SIGNATURE)
}

/// Whether a folder's name says it's a cache: `cache`, `Caches`, npm's
/// `_cacache`, Dart's `.pub-cache`, `marketplace-cache`, Chromium's `Code
/// Cache` and `GPUCache`, and `~/.cache`, where the XDG spec puts caches.
fn named_cache(name: &str) -> bool {
    let n = name.trim_start_matches(['.', '_']).to_ascii_lowercase();
    matches!(n.as_str(), "cache" | "caches" | "cacache")
        || ["-cache", "_cache", ".cache", " cache"]
            .iter()
            .any(|end| n.ends_with(end))
        || name.ends_with("Cache")
}

/// Whether `dir` is a tool's download cache: what it keeps so it downloads
/// nothing twice, and fetches again when it's gone. Only its tool can say
/// (a cache and an install look the same), and it does: it tagged the
/// folder with a `CACHEDIR.TAG` (the standard tar, borg and restic leave
/// out, and Time Machine doesn't), or named it a cache ([`named_cache`]).
/// Only in a tool's folder ([`tool_folder`]); in a git checkout there
/// (Flutter's `bin/cache`), only what git ignores and tracks nothing in.
pub fn download_cache(dir: &Path, home: &Path) -> bool {
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if tool_folder(dir, home).is_none()
        || !privacy::may_open(dir)
        || !std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir())
        || !(named_cache(name) || tagged_cache(dir))
    {
        return false;
    }
    let in_checkout = dir
        .ancestors()
        .skip(1)
        .take_while(|a| a.starts_with(home) && *a != home)
        .any(|a| a.join(".git").symlink_metadata().is_ok());
    !in_checkout
        || worktrees::ignores(dir) == Some(true) && worktrees::tracks_files_in(dir) == Some(false)
}

/// Which of `paths` Time Machine backs up, by `tmutil isexcluded`: the
/// settings' list, macOS's own exclusions and the mark on each item. Those
/// it didn't answer for are missing.
pub fn backed_up(paths: &[&Path]) -> HashMap<PathBuf, bool> {
    let mut out = HashMap::new();
    for chunk in paths.chunks(200) {
        let Ok(o) = Command::new("tmutil")
            .arg("isexcluded")
            .args(chunk)
            .stderr(Stdio::null())
            .output()
        else {
            continue;
        };
        out.extend(parse_isexcluded(&String::from_utf8_lossy(&o.stdout)));
    }
    out
}

/// `[Excluded]    /path` or `[Included]    /path`, a line each.
pub fn parse_isexcluded(out: &str) -> Vec<(PathBuf, bool)> {
    out.lines()
        .filter_map(|l| {
            let (tag, path) = l.split_once(']')?;
            let backed_up = match tag.trim_start_matches('[') {
                "Included" => true,
                "Excluded" => false,
                _ => return None,
            };
            Some((PathBuf::from(path.trim_start()), backed_up))
        })
        .collect()
}

/// What Time Machine does with this Mac, for `mdif timemachine`.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// Whether it has a backup disk.
    pub set_up: bool,
    /// Its settings are there but couldn't be read (no Full Disk Access):
    /// whether it is set up can't be told.
    pub unreadable: bool,
    /// Whether it backs up by itself.
    pub auto: bool,
    /// Seconds between automatic backups.
    pub interval: i64,
    pub destinations: Vec<Destination>,
    /// When the last backup finished.
    pub last_backup: Option<i64>,
    pub running: Option<Running>,
    /// The latest tries, newest first.
    pub tries: Vec<Try>,
    pub backlog: Option<Backlog>,
    /// Oldest first.
    pub snapshots: Vec<Snapshot>,
    /// What macOS counts as purgeable: the snapshots hold at most that.
    pub held: u64,
    /// The button that deletes them.
    pub delete: Option<Action>,
    /// What changed most since the last backup, biggest first.
    pub changes: Vec<Change>,
    /// Folders a build, an install or their tool makes again, biggest
    /// first; those kept out already aren't measured (0).
    pub rebuildable: Vec<Folder>,
    /// What the settings leave out, whole paths.
    pub skipped: Vec<String>,
}

/// The backups and the snapshots, without the folders: a few
/// milliseconds, for a glance (the Overview's alert).
pub fn glance(home: &Path, now: i64) -> Report {
    let (prefs, unreadable) = match prefs() {
        Ok(p) => (p.unwrap_or_default(), false),
        Err(_) => (Prefs::default(), true),
    };
    let set_up = !prefs.destinations.is_empty();
    let mut finished: Vec<i64> = Vec::new();
    let mut attempts: Vec<i64> = Vec::new();
    for d in &prefs.destinations {
        finished.extend(&d.finished);
        attempts.extend(&d.attempts);
    }
    finished.sort();
    attempts.sort();
    let all = tries(&finished, &attempts);
    let running = running();

    let volume = crate::map::boot_volume();
    let names = crate::apfs::local_snapshots(&volume).unwrap_or_default();
    // Each backup disk keeps the one its next backup starts from.
    let references: Vec<i64> = prefs
        .destinations
        .iter()
        .filter_map(|d| d.reference)
        .collect();
    let snaps = snapshots(&names, &references);
    let free = crate::os::fs::statfs(&volume).map_or(0, |s| s.avail);
    let held = if snaps.is_empty() {
        0
    } else {
        space::purgeable(&volume, free)
    };
    let backlog = if set_up && prefs.auto {
        worst_backlog(
            &prefs.destinations,
            prefs.interval,
            running.is_some(),
            held,
            now,
        )
    } else {
        None
    };

    let skipped = settings_skips(&prefs.skip, home)
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    Report {
        set_up,
        unreadable,
        auto: prefs.auto,
        interval: prefs.interval,
        destinations: prefs.destinations,
        last_backup: finished.last().copied(),
        running,
        tries: all
            .into_iter()
            .filter(|t| now - t.started <= TRIES_SPAN)
            .take(TRIES)
            .collect(),
        backlog,
        delete: clean::snapshots_action(&names),
        snapshots: snaps,
        held,
        changes: Vec::new(),
        rebuildable: Vec::new(),
        skipped,
    }
}

/// Everything above, now: the [`glance`], and the folders. `store` is the
/// watcher's history, for what changed since the last backup; `measured`,
/// the last whole-disk measurement, for the sizes of the big folders (those
/// it doesn't keep are measured now).
pub fn report(
    home: &Path,
    store: Option<&Store>,
    keep: &Keep,
    measured: Option<&space::Measured>,
    now: i64,
) -> Report {
    let mut r = glance(home, now);

    // What the next backup sends, and what went since the last.
    let mut changes: Vec<Change> = match (store, r.last_backup) {
        (Some(s), Some(since)) => s
            .growth_since(since)
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, b)| b.abs() >= CHANGE_MIN)
            // Grew, then went before the watcher saw it go: none of it
            // goes into the next backup.
            .filter(|(path, b)| {
                let p = Path::new(path);
                *b < 0 || !privacy::may_open(p) || p.symlink_metadata().is_ok()
            })
            .map(|(path, bytes)| {
                let p = Path::new(&path);
                let known = crate::knowledge::known(p, home);
                let unexplained = (known.is_none() && crate::knowledge::note(p, home).is_none())
                    .then(|| crate::knowledge::generic(p, home))
                    .flatten();
                let known = known.or_else(|| unexplained.as_deref().and_then(crate::ask::answered));
                (path, bytes, known, unexplained)
            })
            .map(|(path, bytes, known, unexplained)| Change {
                name: crate::knowledge::called(Path::new(&path), home),
                known,
                unexplained,
                path,
                bytes,
                backed_up: None,
                kept_out: 0,
            })
            .collect(),
        _ => Vec::new(),
    };
    changes.sort_by_key(|c| std::cmp::Reverse(c.bytes.abs()));
    changes.truncate(CHANGES);

    // Each folder checked once, side by side (git for most); only those
    // still backed up are measured: the others are a count.
    let mut found: Vec<(PathBuf, &'static str)> = crate::tidy::artifact_folders(home)
        .into_par_iter()
        .filter(|p| !keep.touches(p))
        .filter_map(|p| rebuildable(&p).map(|what| (p, what)))
        .collect::<Vec<_>>()
        .into_iter()
        .chain(
            crate::tidy::tool_caches(home)
                .into_iter()
                .filter(|p| !keep.touches(p))
                .map(|p| (p, "cache")),
        )
        .collect();

    if let Some(s) = store {
        let more = kept_out_by_mdif(s, home, &found);
        found.extend(more);
    }

    let asked: Vec<PathBuf> = changes
        .iter()
        .map(|c| PathBuf::from(&c.path))
        .chain(found.iter().map(|(p, _)| p.clone()))
        .collect();
    let refs: Vec<&Path> = asked.iter().map(PathBuf::as_path).collect();
    let state = if r.set_up {
        backed_up(&refs)
    } else {
        HashMap::new()
    };
    for c in &mut changes {
        c.backed_up = state.get(Path::new(&c.path)).copied();
    }
    let mut rebuildable: Vec<Folder> = found
        .into_par_iter()
        .map(|(p, what)| {
            let backed_up = state
                .get(&p)
                .copied()
                .unwrap_or_else(|| !crate::os::fs::excluded_from_backup(&p).unwrap_or(false));
            let (checkout, project) = match what {
                "cache" => {
                    let tool = tool_folder(&p, home).unwrap_or_else(|| p.clone());
                    (tool.clone(), tool)
                }
                _ => project(&p, home),
            };
            Folder {
                bytes: match backed_up {
                    true => measured
                        .and_then(|m| m.size_at(&p))
                        .or_else(|| scan::size_of(&p))
                        .unwrap_or(0),
                    false => 0,
                },
                path: p.to_string_lossy().into_owned(),
                what,
                backed_up,
                checkout: checkout.to_string_lossy().into_owned(),
                project: project.to_string_lossy().into_owned(),
            }
        })
        .collect();
    rebuildable.retain(|f| f.what != "cache" || !f.backed_up || f.bytes >= CACHE_MIN);
    rebuildable.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.path.cmp(&b.path)));
    for c in &mut changes {
        c.kept_out = kept_out(Path::new(&c.path), &rebuildable);
    }

    r.changes = changes;
    r.rebuildable = rebuildable;
    r
}

/// Where the folders mdif kept out of the backups are listed: a JSON list
/// of `[path, what, inode]`. The inode finds one renamed or moved since (a
/// `dist` renamed to keep a release, which takes its exclusion with it), so
/// that it's still on the page, to be let back in.
pub const EXCLUDED_KEY: &str = "timemachine.excluded";

type Excluded = (String, String, u64);

fn excluded_by_mdif(store: &Store) -> Vec<Excluded> {
    store
        .meta(EXCLUDED_KEY)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

/// Notes that mdif kept `paths` out of the backups (`exclude`), or let them
/// back in: for [`kept_out_by_mdif`].
pub fn remember_excluded(
    store: &Store,
    paths: &[PathBuf],
    exclude: bool,
    home: &Path,
) -> Result<()> {
    let mut list = excluded_by_mdif(store);
    for p in paths {
        let path = p.to_string_lossy().into_owned();
        let inode = std::fs::symlink_metadata(p).map_or(0, |m| crate::portable::file_id(p, &m));
        list.retain(|(q, _, i)| *q != path && (inode == 0 || *i != inode));
        if exclude && inode != 0 {
            let what = match rebuildable(p) {
                Some(w) => w,
                None if download_cache(p, home) => "cache",
                None => "build",
            };
            list.push((path, what.to_string(), inode));
        }
    }
    list.sort();
    store.set_meta(EXCLUDED_KEY, &serde_json::to_string(&list)?)
}

/// The folders mdif kept out that aren't among `found`, where they are now
/// (renamed or moved in the home folder): still folders, and still kept out.
/// One let back in by hand, or deleted, isn't listed.
fn kept_out_by_mdif(
    store: &Store,
    home: &Path,
    found: &[(PathBuf, &'static str)],
) -> Vec<(PathBuf, &'static str)> {
    let volume = crate::os::fs::fsid(home).ok();
    let mut out: Vec<(PathBuf, &'static str)> = Vec::new();
    for (path, what, inode) in excluded_by_mdif(store) {
        let what = match what.as_str() {
            "dependencies" => "dependencies",
            "cache" => "cache",
            _ => "build",
        };
        let same = |p: &Path| {
            std::fs::symlink_metadata(p).is_ok_and(|m| crate::portable::file_id(p, &m) == inode)
        };
        let path = PathBuf::from(path);
        let now = if privacy::may_open(&path) && same(&path) {
            Some(path)
        } else {
            volume
                .and_then(|v| crate::os::fs::path_of(v, inode).ok())
                .filter(|p| p.starts_with(home))
        };
        let Some(p) = now else { continue };
        if !found.iter().any(|(f, _)| *f == p)
            && !out.iter().any(|(f, _)| *f == p)
            && privacy::may_open(&p)
            && std::fs::symlink_metadata(&p)
                .is_ok_and(|m| m.is_dir() && crate::portable::file_id(&p, &m) == inode)
            && crate::os::fs::excluded_from_backup(&p).unwrap_or(false)
        {
            out.push((p, what));
        }
    }
    out
}

/// How many of `folders` inside `dir` Time Machine skips.
fn kept_out(dir: &Path, folders: &[Folder]) -> usize {
    folders
        .iter()
        .filter(|f| !f.backed_up && Path::new(&f.path).starts_with(dir))
        .count()
}

/// Keeps `paths` out of the backups (`exclude`) or lets them back in: only
/// folders a build or an install makes again ([`rebuildable`]) or their
/// tool downloads again ([`download_cache`]) go out, and never what you
/// keep or what has something you keep in it. Any folder Time Machine
/// skips may come back in, one that no longer looks made again too (a
/// `dist` renamed to keep a release). One let back in is marked
/// ([`LET_IN`]), and so is the name on its folder. One System Settings
/// keeps out isn't let in: its list wins. Each comes back done, or refused
/// and why.
pub fn set_excluded(
    paths: &[PathBuf],
    exclude: bool,
    keep: &Keep,
    home: &Path,
) -> Vec<(PathBuf, Result<(), String>)> {
    let skips = match exclude {
        true => Vec::new(),
        false => prefs()
            .ok()
            .flatten()
            .map(|p| settings_skips(&p.skip, home))
            .unwrap_or_default(),
    };
    set_excluded_with(paths, exclude, keep, home, &skips)
}

fn set_excluded_with(
    paths: &[PathBuf],
    exclude: bool,
    keep: &Keep,
    home: &Path,
    skips: &[PathBuf],
) -> Vec<(PathBuf, Result<(), String>)> {
    paths
        .iter()
        .map(|p| {
            let shown = crate::text::printable(&p.to_string_lossy()).into_owned();
            let skipped = || {
                std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
                    && crate::os::fs::excluded_from_backup(p).unwrap_or(false)
            };
            let done = if exclude && keep.covering(p).is_some() {
                Err(format!("{shown} is on your keep list."))
            } else if let Some(s) = skips.iter().find(|s| p.starts_with(s)) {
                Err(format!(
                    "Time Machine's own settings keep {} out: take it off the list in System \
                     Settings › General › Time Machine › Options.",
                    crate::text::printable(&s.to_string_lossy())
                ))
            } else if exclude && keep.touches(p) {
                Err(format!("{shown} has something on your keep list in it."))
            } else if rebuildable(p).is_none()
                && !download_cache(p, home)
                && (exclude || !skipped())
            {
                Err(format!(
                    "{shown} isn't a folder a build, an install or its tool makes again."
                ))
            } else {
                crate::os::fs::set_excluded_from_backup(p, exclude)
                    .and_then(|()| crate::os::fs::set_mark(p, LET_IN, !exclude))
                    .map_err(|e| format!("{shown}: {e}"))
                    .inspect(|()| {
                        if let (Some(parent), Some(mark)) =
                            (p.parent(), p.file_name().and_then(let_in_child))
                        {
                            let _ = crate::os::fs::set_mark(parent, &mark, !exclude);
                        }
                    })
            };
            (p.clone(), done)
        })
        .collect()
}

/// `paths` as [`set_excluded`] takes them: a folder made again as it is, a
/// project or a tool's folder (or any folder) as the ones inside it; one
/// with none inside as it is, to be refused.
pub fn expand(paths: &[PathBuf], home: &Path) -> Vec<PathBuf> {
    let mut found: Option<Vec<PathBuf>> = None;
    let mut out = Vec::new();
    for p in paths {
        if rebuildable(p).is_some() || download_cache(p, home) || !p.is_dir() {
            out.push(p.clone());
            continue;
        }
        let all = found.get_or_insert_with(|| {
            let mut all: Vec<PathBuf> = crate::tidy::artifact_folders(home)
                .into_par_iter()
                .filter(|f| rebuildable(f).is_some())
                .collect();
            all.extend(crate::tidy::tool_caches(home));
            all
        });
        let inside: Vec<PathBuf> = all.iter().filter(|f| f.starts_with(p)).cloned().collect();
        if inside.is_empty() {
            out.push(p.clone());
        } else {
            out.extend(inside);
        }
    }
    out
}

/// Starts a backup now, as Back Up Now in the menu does.
pub fn back_up_now() -> Result<(), String> {
    let out = Command::new("tmutil")
        .arg("startbackup")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("tmutil: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if why.is_empty() {
            "Time Machine didn't start a backup.".into()
        } else {
            why
        })
    }
}

// What these tests assert is macOS: its folders, tools and file system.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn the_apps_time_machine_fixture_has_the_shape_report_prints() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../app/Tests/timemachine.json")).unwrap();
        let real = serde_json::to_value(Report {
            set_up: true,
            unreadable: false,
            auto: true,
            interval: DAY,
            destinations: vec![Destination {
                name: "Backups".into(),
                server: Some("NAS".into()),
                quota: Some(1),
                used: Some(1),
                available: Some(1),
                ..Destination::default()
            }],
            last_backup: Some(1),
            running: Some(Running {
                done: Some(0.5),
                phase: Some("Copying".into()),
            }),
            tries: vec![Try {
                started: 1,
                finished: None,
            }],
            backlog: Some(Backlog {
                since: Some(1),
                failed: 2,
                held: 1,
                to: Some(String::new()),
            }),
            snapshots: vec![Snapshot {
                name: String::new(),
                taken: 1,
                reference: true,
            }],
            held: 1,
            delete: clean::snapshots_action(&[
                "com.apple.TimeMachine.2026-10-27-231632.local".to_string()
            ]),
            changes: vec![Change {
                path: String::new(),
                name: None,
                known: None,
                unexplained: None,
                bytes: -1,
                backed_up: Some(true),
                kept_out: 0,
            }],
            rebuildable: vec![Folder {
                path: String::new(),
                what: "build",
                bytes: 1,
                backed_up: true,
                checkout: String::new(),
                project: String::new(),
            }],
            skipped: vec![String::new()],
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
        for list in [
            "destinations",
            "tries",
            "snapshots",
            "changes",
            "rebuildable",
        ] {
            for item in fixture[list].as_array().unwrap() {
                assert_eq!(keys(item), keys(&real[list][0]), "{list}");
            }
        }
        for one in ["running", "backlog", "delete"] {
            assert_eq!(keys(&fixture[one]), keys(&real[one]), "{one}");
        }
    }

    #[test]
    fn a_network_disk_is_named_by_its_server_never_its_account() {
        assert_eq!(
            server("smb://GUEST:@NAS._smb._tcp.local./timemachine").as_deref(),
            Some("NAS")
        );
        assert_eq!(server("smb://me:pw@nas.local/tm").as_deref(), Some("nas"));
        assert_eq!(
            server("afp://192.168.1.5:548/tm").as_deref(),
            Some("192.168.1.5")
        );
        assert_eq!(server("not a url"), None);
    }

    #[test]
    fn a_try_finished_when_a_backup_did_before_the_next_try() {
        // As on the author's Mac: a backup on the 27th, four tries that
        // didn't finish, then one that did.
        let attempts = [100, 1000, 2000, 3000, 4000];
        let finished = [50, 150, 4500];
        let t = tries(&finished, &attempts);
        let got: Vec<(i64, Option<i64>)> = t.iter().map(|t| (t.started, t.finished)).collect();
        assert_eq!(
            got,
            [
                (4000, Some(4500)),
                (3000, None),
                (2000, None),
                (1000, None),
                (100, Some(150)),
                (50, Some(50)),
            ]
        );
    }

    #[test]
    fn backups_are_failing_after_two_tries_or_twice_the_interval() {
        let now = 100 * DAY;
        let ok = |at| Try {
            started: at,
            finished: Some(at + 600),
        };
        let failed = |at| Try {
            started: at,
            finished: None,
        };
        // Fine: the last one finished an hour ago.
        assert_eq!(backlog(&[ok(now - HOUR)], HOUR, false, 0, now), None);
        // One that didn't finish is no pattern yet.
        assert_eq!(
            backlog(&[failed(now - 60), ok(now - 2 * HOUR)], HOUR, false, 0, now),
            None
        );
        let b = backlog(
            &[failed(now - 60), failed(now - HOUR), ok(now - 2 * DAY)],
            DAY,
            false,
            5,
            now,
        )
        .unwrap();
        assert_eq!(
            (b.since, b.failed, b.held),
            (Some(now - 2 * DAY + 600), 2, 5)
        );
        // The one under way isn't a failure.
        assert_eq!(
            backlog(
                &[failed(now - 60), failed(now - HOUR), ok(now - 3 * HOUR)],
                HOUR,
                true,
                0,
                now
            ),
            None
        );
        // Daily backups: two days without one is overdue, one isn't.
        assert_eq!(backlog(&[ok(now - DAY - HOUR)], DAY, false, 0, now), None);
        let late = backlog(&[ok(now - 3 * DAY)], DAY, false, 0, now).unwrap();
        // Overdue, nothing failed: said so, not "failing".
        assert_eq!(late.failed, 0);
        // The lid opened on Monday, the backup under way: catching up.
        assert_eq!(
            backlog(&[failed(now - 60), ok(now - 3 * DAY)], HOUR, true, 0, now),
            None
        );
    }

    /// A USB disk backing up every hour, a NAS out of reach for two weeks:
    /// merged, the NAS's backlog would hide behind the USB disk's backups.
    #[test]
    fn each_backup_disk_is_judged_by_its_own_backups() {
        let now = 1_800_000_000;
        let hourly: Vec<i64> = (1..48).map(|h| now - h * HOUR).collect();
        let usb = Destination {
            name: "USB".into(),
            finished: hourly.clone(),
            attempts: hourly.iter().map(|f| f - 600).collect(),
            reference: Some(now - HOUR),
            ..Destination::default()
        };
        let nas = Destination {
            name: "NAS".into(),
            finished: vec![now - 14 * DAY],
            attempts: vec![now - 14 * DAY - 600],
            reference: Some(now - 14 * DAY),
            ..Destination::default()
        };
        let both = [usb.clone(), nas.clone()];
        let b = worst_backlog(&both, HOUR, false, 7, now).unwrap();
        assert_eq!(
            (b.to.as_deref(), b.since, b.held),
            (Some("NAS"), Some(now - 14 * DAY), 7)
        );
        // One disk, nothing to name; a healthy one, nothing to say.
        assert_eq!(worst_backlog(&[nas], HOUR, false, 7, now).unwrap().to, None);
        assert_eq!(worst_backlog(&[usb], HOUR, false, 7, now), None);
        // Each disk's reference snapshot is held, not only the newest.
        let names = [
            "com.apple.TimeMachine.2026-10-15-093349.local".to_string(),
            "com.apple.TimeMachine.2026-10-26-231632.local".to_string(),
            "com.apple.TimeMachine.2026-10-29-081500.local".to_string(),
        ];
        let taken: Vec<i64> = snapshots(&names, &[]).iter().map(|s| s.taken).collect();
        let refs = [taken[0] + 20, taken[2] - 5];
        let held: Vec<bool> = snapshots(&names, &refs)
            .iter()
            .map(|s| s.reference)
            .collect();
        assert_eq!(held, [true, false, true]);
    }

    #[test]
    fn tmutil_says_what_it_backs_up_and_what_not() {
        let out = "[Excluded]    /Users/me/Library/Caches\n[Included]    /Users/me/devel/a b/build\nnoise\n";
        assert_eq!(
            parse_isexcluded(out),
            [
                (PathBuf::from("/Users/me/Library/Caches"), false),
                (PathBuf::from("/Users/me/devel/a b/build"), true),
            ]
        );
    }

    #[test]
    fn a_change_counts_the_build_folders_kept_out_inside_it() {
        let folder = |path: &str, backed_up| Folder {
            path: path.into(),
            what: "dependencies",
            bytes: 0,
            backed_up,
            checkout: String::new(),
            project: String::new(),
        };
        let folders = [
            folder("/w/agent-1/node_modules", false),
            folder("/w/agent-1/apps/web/node_modules", false),
            folder("/w/agent-1/apps/web/dist", true),
            folder("/w/agent-10/node_modules", false),
        ];
        assert_eq!(kept_out(Path::new("/w/agent-1"), &folders), 2);
        assert_eq!(kept_out(Path::new("/w/agent-2"), &folders), 0);
    }

    #[test]
    fn a_folder_belongs_to_its_repository_worktrees_inside_it_too() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let mk = |p: &str| std::fs::create_dir_all(h.join(p)).unwrap();
        mk("devel/app/.git");
        mk("devel/app/apps/web/dist");
        mk("devel/app/.claude/worktrees/w/node_modules");
        std::fs::write(h.join("devel/app/.claude/worktrees/w/.git"), "gitdir: x").unwrap();
        mk("devel/elsewhere/.git");
        mk("devel/elsewhere-wt/build");
        std::fs::write(h.join("devel/elsewhere-wt/.git"), "gitdir: x").unwrap();
        mk("loose/x/build");

        let at = |p: &str| {
            let (checkout, project) = project(&h.join(p), h);
            (
                checkout.strip_prefix(h).unwrap().to_owned(),
                project.strip_prefix(h).unwrap().to_owned(),
            )
        };
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(
            at("devel/app/apps/web/dist"),
            (p("devel/app"), p("devel/app"))
        );
        assert_eq!(
            at("devel/app/.claude/worktrees/w/node_modules"),
            (p("devel/app/.claude/worktrees/w"), p("devel/app"))
        );
        assert_eq!(
            at("devel/elsewhere-wt/build"),
            (p("devel/elsewhere-wt"), p("devel/elsewhere-wt"))
        );
        assert_eq!(at("loose/x/build"), (p("loose/x"), p("loose/x")));
    }

    #[test]
    fn a_backup_under_way_says_how_far_it_is() {
        let idle = "Backup session status:\n{\n    ClientID = \"com.apple.backupd\";\n    Percent = \"-1\";\n    Running = 0;\n}\n";
        assert_eq!(parse_status(idle), None);
        let busy = "Backup session status:\n{\n    BackupPhase = Copying;\n    Percent = \"0.42\";\n    Running = 1;\n}\n";
        assert_eq!(
            parse_status(busy),
            Some(Running {
                done: Some(0.42),
                phase: Some("Copying".into())
            })
        );
    }

    #[test]
    fn the_settings_say_where_how_often_and_what_finished() {
        let mut dest = plist::Dictionary::new();
        dest.insert("LastKnownVolumeName".into(), "Backup".into());
        dest.insert(
            "NetworkURL".into(),
            "smb://GUEST:@NAS._smb._tcp.local./tm".into(),
        );
        dest.insert("QuotaGB".into(), 649.into());
        dest.insert("BytesUsed".into(), 482_580_426_752u64.into());
        let date = |s: u64| {
            plist::Value::Date((std::time::UNIX_EPOCH + std::time::Duration::from_secs(s)).into())
        };
        dest.insert(
            "SnapshotDates".into(),
            plist::Value::Array(vec![date(300), date(100)]),
        );
        dest.insert("AttemptDates".into(), plist::Value::Array(vec![date(250)]));
        dest.insert("ReferenceLocalSnapshotDate".into(), date(250));
        let mut root = plist::Dictionary::new();
        root.insert("AutoBackup".into(), true.into());
        root.insert("AutoBackupInterval".into(), 86_400.into());
        root.insert(
            "SkipPaths".into(),
            plist::Value::Array(vec!["/Users/me/Downloads".into()]),
        );
        root.insert(
            "Destinations".into(),
            plist::Value::Array(vec![dest.into()]),
        );
        let mut bytes = Vec::new();
        plist::Value::Dictionary(root)
            .to_writer_binary(&mut bytes)
            .unwrap();
        let p = parse_prefs(&bytes).unwrap();
        assert!(p.auto);
        assert_eq!(p.interval, DAY);
        assert_eq!(p.skip, ["/Users/me/Downloads"]);
        let d = &p.destinations[0];
        assert_eq!(d.name, "Backup");
        assert_eq!(d.server.as_deref(), Some("NAS"));
        assert_eq!(d.quota, Some(649_000_000_000));
        assert_eq!(d.used, Some(482_580_426_752));
        assert_eq!(d.finished, [100, 300], "oldest first");
        assert_eq!(d.attempts, [250]);
        assert_eq!(d.reference, Some(250));

        // Hourly when the settings don't say.
        let mut bytes = Vec::new();
        plist::Value::Dictionary(plist::Dictionary::new())
            .to_writer_binary(&mut bytes)
            .unwrap();
        assert_eq!(parse_prefs(&bytes).unwrap().interval, HOUR);
    }

    #[test]
    fn only_time_machines_snapshots_are_listed_and_the_reference_marked() {
        let names = [
            "com.apple.TimeMachine.2026-10-29-093349.local".to_string(),
            "com.apple.TimeMachine.2026-10-27-231632.local".to_string(),
            "com.apple.os.update-MSUPrepareUpdate".to_string(),
        ];
        let first = snapshots(&names, &[]);
        assert_eq!(first.len(), 2);
        assert!(first[0].taken < first[1].taken, "oldest first");
        let reference = first[1].taken + 20;
        let s = snapshots(&names, &[reference]);
        let marked: Vec<bool> = s.iter().map(|s| s.reference).collect();
        assert_eq!(marked, [false, true]);
    }

    #[test]
    fn build_output_git_ignores_is_rebuildable_what_git_tracks_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        let repo = home.join("devel/app");
        std::fs::create_dir_all(repo.join("ios/build/Debug")).unwrap();
        std::fs::create_dir_all(repo.join("docs/build")).unwrap();
        std::fs::create_dir_all(repo.join("notes")).unwrap();
        std::fs::write(repo.join("ios/build/Debug/App"), b"x").unwrap();
        std::fs::write(repo.join("docs/build/index.html"), b"x").unwrap();
        std::fs::write(repo.join(".gitignore"), b"ios/build/\nnotes/\n").unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(args)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .unwrap()
                    .success(),
                "git {args:?}"
            );
        };
        git(&["init", "-q"]);
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "t",
        ]);
        // Xcode's build folder, ignored: made again by the next build.
        assert_eq!(rebuildable(&repo.join("ios/build")), Some("build"));
        // A folder named build that git tracks: someone's files.
        assert_eq!(rebuildable(&repo.join("docs/build")), None);
        // Ignored, but not named as build output.
        assert_eq!(rebuildable(&repo.join("notes")), None);
        // Refused, with the reason, and nothing marked.
        let keep = Keep::default();
        let r = set_excluded(&[repo.join("docs/build")], true, &keep, &home);
        assert!(r[0].1.is_err());
        assert!(!crate::os::fs::excluded_from_backup(&repo.join("docs/build")).unwrap());
        let r = set_excluded(&[repo.join("ios/build")], true, &keep, &home);
        assert_eq!(r[0].1, Ok(()));
        assert!(crate::os::fs::excluded_from_backup(&repo.join("ios/build")).unwrap());
        let r = set_excluded(&[repo.join("ios/build")], false, &keep, &home);
        assert_eq!(r[0].1, Ok(()));
        assert!(!crate::os::fs::excluded_from_backup(&repo.join("ios/build")).unwrap());
        // A project stands for its folders a build makes again; a folder
        // with none in it stays itself, to be refused.
        assert_eq!(
            expand(std::slice::from_ref(&repo), &home),
            [repo.join("ios/build")]
        );
        assert_eq!(expand(&[repo.join("notes")], &home), [repo.join("notes")]);
    }

    #[test]
    fn what_mdif_kept_out_is_found_again_where_it_went() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        let app = home.join("devel/app");
        let dist = app.join("dist");
        let kept = app.join("node_modules");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::create_dir_all(&kept).unwrap();
        let store = Store::open_in_memory().unwrap();
        for p in [&dist, &kept] {
            crate::os::fs::set_excluded_from_backup(p, true).unwrap();
        }
        remember_excluded(&store, &[dist.clone(), kept.clone()], true, &home).unwrap();
        // Found as made again: listed there, not twice.
        let found = [(kept.clone(), "dependencies")];
        assert_eq!(
            kept_out_by_mdif(&store, &home, &found),
            [(dist.clone(), "build")]
        );
        // Renamed to keep a release: found where it is now.
        let release = home.join("Archive/release-1.0");
        std::fs::create_dir_all(release.parent().unwrap()).unwrap();
        std::fs::rename(&dist, &release).unwrap();
        assert_eq!(
            kept_out_by_mdif(&store, &home, &found),
            [(release.clone(), "build")]
        );
        // Let back in: no longer listed, by mdif or by hand.
        crate::os::fs::set_excluded_from_backup(&release, false).unwrap();
        assert!(kept_out_by_mdif(&store, &home, &found).is_empty());
        crate::os::fs::set_excluded_from_backup(&release, true).unwrap();
        remember_excluded(&store, std::slice::from_ref(&release), false, &home).unwrap();
        assert!(kept_out_by_mdif(&store, &home, &found).is_empty());
        // Deleted: gone from the list, and nothing else taken for it.
        std::fs::remove_dir(&kept).unwrap();
        assert!(kept_out_by_mdif(&store, &home, &[]).is_empty());
    }

    #[test]
    fn what_holds_a_kept_folder_stays_in_and_a_renamed_one_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        let app = home.join("devel/app");
        std::fs::create_dir_all(app.join("target/release")).unwrap();
        std::fs::create_dir_all(app.join("notes")).unwrap();
        std::fs::write(app.join("Cargo.toml"), "").unwrap();
        let target = app.join("target");
        // A build of it you keep is inside: it stays backed up.
        let keep = Keep::parse("~/devel/app/target/release\n", &home);
        let r = set_excluded(std::slice::from_ref(&target), true, &keep, &home);
        let why = r[0].1.as_ref().unwrap_err();
        assert!(why.contains("keep list in it"), "{why}");
        // Kept out, then renamed to keep it: no longer build output, and
        // still it may come back in.
        let none = Keep::default();
        let r = set_excluded(std::slice::from_ref(&target), true, &none, &home);
        assert_eq!(r[0].1, Ok(()));
        let release = app.join("release-1.0");
        std::fs::rename(&target, &release).unwrap();
        assert!(rebuildable(&release).is_none());
        let r = set_excluded(std::slice::from_ref(&release), false, &none, &home);
        assert_eq!(r[0].1, Ok(()));
        // One Time Machine backs up isn't marked for it by name.
        let r = set_excluded(&[app.join("notes")], false, &none, &home);
        assert!(r[0].1.is_err());
        // Let back in, then deleted and made again by the next build: still
        // let in, by the name its folder keeps.
        std::fs::rename(&release, &target).unwrap();
        let r = set_excluded(std::slice::from_ref(&target), true, &none, &home);
        assert_eq!(r[0].1, Ok(()));
        assert!(!let_in(&target));
        let r = set_excluded(std::slice::from_ref(&target), false, &none, &home);
        assert_eq!(r[0].1, Ok(()));
        std::fs::remove_dir_all(&target).unwrap();
        std::fs::create_dir_all(target.join("release")).unwrap();
        assert!(let_in(&target));
        assert!(!let_in(&app.join("notes")));
        // System Settings' own list keeps it out: not let in, and said where.
        let skips = settings_skips(&["~/devel/app".into()], &home);
        assert_eq!(skips, [home.join("devel/app")]);
        let r = set_excluded_with(std::slice::from_ref(&target), false, &none, &home, &skips);
        let why = r[0].1.as_ref().unwrap_err();
        assert!(why.contains("System Settings"), "{why}");
    }

    #[test]
    fn a_snapshot_named_in_local_time_is_dated_right_around_summer_time() {
        // Every quarter hour of 2026, in this Mac's time zone: its local
        // name read back is the same moment, but in the hour the clocks go
        // back, which two moments share.
        let start = 1_767_225_600; // 2026-01-01 UTC
        let wall = |t: i64| t + crate::os::sys::utc_offset(t);
        for t in (start..start + 365 * 86_400).step_by(900) {
            let w = wall(t);
            let shared = [-3600, 3600].iter().any(|d| wall(t + d) == w);
            if !shared {
                assert_eq!(from_wall(w), t, "{t}");
            }
        }
    }

    #[test]
    fn a_tool_says_which_of_its_folders_are_caches() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        for d in [
            ".bun/install/cache/react@18.2.0@@@1",
            ".bun/bin",
            ".cargo/registry/src",
            ".cargo/bin",
            ".pub-cache/hosted/pub.dev",
            ".cache/uv",
            ".tool/cache-notes",
            ".gradle/caches",
            ".sdk/bin/cache",
            ".sdk/lib/cache",
            "Documents/cache",
            "devel/app/cache",
            "go/pkg/mod/cache",
            "Research/scraper/cache",
            "Dev/app/cache",
            ".tool/downloads",
        ] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        std::fs::write(
            home.join(".cargo/registry/CACHEDIR.TAG"),
            b"Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag.\n",
        )
        .unwrap();
        // Named like the tag, without what the standard says is in it.
        std::fs::write(home.join(".tool/downloads/CACHEDIR.TAG"), b"x").unwrap();
        // A tool's own checkout: what git ignores in it is a cache, what it
        // tracks isn't.
        let sdk = home.join(".sdk");
        std::fs::write(sdk.join("lib/cache/kept.txt"), b"x").unwrap();
        std::fs::write(sdk.join(".gitignore"), b"bin/cache/\n").unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&sdk)
                    .args(args)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .unwrap()
                    .success(),
                "git {args:?}"
            );
        };
        git(&["init", "-q"]);
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "t",
        ]);
        std::fs::create_dir_all(sdk.join(".git/lfs/cache")).unwrap();
        // Kept out already: so is what's in it.
        crate::os::fs::set_excluded_from_backup(&home.join(".gradle"), true).unwrap();

        // Tagged, named one by its tool, or in an XDG cache home; the
        // outermost only. Not in .git, macOS's or your folders (a research
        // project's `cache` is yours, `~/Dev` a code folder in any case), or
        // projects.
        let found: Vec<PathBuf> = [
            ".bun/install/cache",
            ".cache",
            ".cargo/registry",
            ".pub-cache",
            ".sdk/bin/cache",
            "go/pkg/mod/cache",
        ]
        .iter()
        .map(|d| home.join(d))
        .collect();
        assert_eq!(crate::tidy::tool_caches(&home), found);
        assert_eq!(
            tool_folder(&home.join(".bun/install/cache"), &home),
            Some(home.join(".bun"))
        );
        assert_eq!(tool_folder(&home.join("Documents/cache"), &home), None);
        assert_eq!(
            tool_folder(&home.join("Research/scraper/cache"), &home),
            None
        );

        // Only those are marked; a tool's folder stands for its caches.
        let keep = Keep::default();
        let r = set_excluded(&[home.join(".bun/bin")], true, &keep, &home);
        assert!(r[0].1.is_err());
        assert_eq!(
            expand(&[home.join(".bun")], &home),
            [home.join(".bun/install/cache")]
        );
        let cache = home.join(".bun/install/cache");
        let r = set_excluded(std::slice::from_ref(&cache), true, &keep, &home);
        assert_eq!(r[0].1, Ok(()));
        assert!(crate::os::fs::excluded_from_backup(&cache).unwrap());

        // The tidy-up keeps out the big ones, but not one you let back in.
        let pubs = home.join(".pub-cache");
        std::fs::write(cache.join("react.tgz"), vec![1u8; 11_000_000]).unwrap();
        std::fs::write(
            pubs.join("hosted/pub.dev/http.tar.gz"),
            vec![1u8; 11_000_000],
        )
        .unwrap();
        let r = set_excluded(std::slice::from_ref(&cache), false, &keep, &home);
        assert_eq!(r[0].1, Ok(()));
        let o = crate::tidy::run(crate::tidy::Id::Backups, &home, &keep, 0, false);
        let done: Vec<&Path> = o.done.iter().map(|d| d.path.as_path()).collect();
        assert_eq!(done, [pubs.as_path()]);
        assert!(!crate::os::fs::excluded_from_backup(&cache).unwrap());
        assert!(crate::os::fs::excluded_from_backup(&pubs).unwrap());
    }
}
