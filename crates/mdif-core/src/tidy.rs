//! Tidy-ups the watcher does once a day, each only once you turn it on
//! (`mdif tidy`, switches under Prevent in the app): what nothing has used
//! for weeks and that comes back by itself when it's needed.
//!
//! - `simulators`: simulator runtimes not used in 60 days (`simctl runtime
//!   delete`), then the simulators left without a runtime.
//! - `builds`: a project's build output (what [`clean::build_output`]
//!   takes) that nothing has built into or read from for 14 days.
//! - `node-modules`: a project's `node_modules` (what
//!   [`clean::dependencies`] takes) that nothing has installed into or
//!   read from for 14 days.
//! - `incremental`: a Rust build's incremental caches that no build has
//!   touched for 30 days.
//! - `caches`: app and tool caches of 50 MB or more that nothing has written
//!   to or read for 30 days, emptied while their app is closed.
//!
//! Files go only through [`clean::plan`] and [`clean::run`], which check each
//! folder again; what you keep ([`Keep`]) is never touched, and a simulator
//! you keep stays too. Finding them reads metadata only (a file's contents
//! never: reading one would make it look used), never follows a link, never
//! crosses to another volume or into a cloud placeholder, and never opens
//! what would show a privacy prompt ([`privacy::may_open`]).

use std::os::macos::fs::MetadataExt as _;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::clean::{self, Step};
use crate::keep::Keep;
use crate::owners::{ARTIFACTS, CODE_ROOTS};
use crate::privacy;
use crate::space::{run_within, runtime_name, simctl};
use crate::store::Store;
use crate::suggest::display;

const DAY: i64 = 86_400;

/// A tidy-up runs again once this long has passed since it last ran.
pub const EVERY: i64 = DAY;
/// Simulator runtimes not used for this many days go.
pub const SIMULATOR_DAYS: u32 = 60;
/// Build output and `node_modules` nothing has written to or read from for
/// this long go.
pub const BUILD_AGE: i64 = 14 * DAY;
/// Incremental caches no build has touched for this long go.
pub const INCREMENTAL_AGE: i64 = 30 * DAY;
/// Caches nothing has written to or read for this long are emptied ...
pub const CACHE_AGE: i64 = 30 * DAY;
/// ... when they hold at least this much.
pub const CACHE_MIN: u64 = 50_000_000;

/// How many folders below a code folder a project's `target` is looked for.
const PROJECT_DEPTH: usize = 6;

/// Tool caches that hold what the tool doesn't download again by itself
/// (Playwright's and Puppeteer's browsers, LM Studio's models), or only by
/// fetching gigabytes (Hugging Face's models): the button may empty them,
/// the daily tidy-up doesn't.
const NOT_TIDIED: &[&str] = &["ms-playwright", "puppeteer", "lm-studio", "huggingface"];

const NO_XCODE: &str = "Needs Xcode, which isn't installed.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Id {
    Simulators,
    Builds,
    Dependencies,
    Incremental,
    Caches,
}

impl Id {
    pub const ALL: [Id; 5] = [
        Id::Simulators,
        Id::Builds,
        Id::Dependencies,
        Id::Incremental,
        Id::Caches,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Id::Simulators => "simulators",
            Id::Builds => "builds",
            Id::Dependencies => "node-modules",
            Id::Incremental => "incremental",
            Id::Caches => "caches",
        }
    }

    pub fn parse(s: &str) -> Option<Id> {
        Id::ALL.into_iter().find(|id| id.id() == s)
    }

    pub fn title(self) -> &'static str {
        match self {
            Id::Simulators => "Old simulators",
            Id::Builds => "Old build folders",
            Id::Dependencies => "Old node_modules",
            Id::Incremental => "Old Rust build caches",
            Id::Caches => "Caches apps stopped using",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Id::Simulators => {
                "Simulator runtimes not used in 60 days, and the simulators left without one. \
                 Xcode downloads a runtime again when you need it."
            }
            Id::Builds => {
                "A project's build output (Rust's target, Flutter's and Gradle's build, \
                 Swift's .build, .next, dist) that nothing has built into or run from for 14 \
                 days. Its next build makes it again, from scratch."
            }
            Id::Dependencies => {
                "A project's node_modules that nothing has installed into or run from for 14 \
                 days. npm, yarn, pnpm or bun puts it back from the lockfile, downloading what \
                 isn't in its cache."
            }
            Id::Incremental => {
                "A Rust build's incremental caches no build has used for 30 days: the next \
                 build of that project's own crates is slower, nothing else."
            }
            Id::Caches => {
                "App and tool caches nothing has written to for 30 days, emptied while their \
                 app is closed; the app makes what it needs again."
            }
        }
    }

    /// Why it can't run on this Mac, if it can't.
    pub fn unavailable(self) -> Option<String> {
        match self {
            Id::Simulators if simctl().is_none() => Some(NO_XCODE.into()),
            _ => None,
        }
    }

    /// What it does to each thing, done.
    fn verb(self) -> &'static str {
        match self {
            Id::Caches => "emptied",
            _ => "deleted",
        }
    }
}

/// What a tidy-up did the last time it ran.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Last {
    /// Unix seconds.
    pub at: i64,
    pub freed: u64,
    pub done: usize,
    pub failed: usize,
    /// "Deleted the iOS 26.0 Simulator", "Emptied 4 caches", "Nothing to tidy".
    pub note: Option<String>,
}

/// A tidy-up as `mdif tidy --json` lists it.
#[derive(Debug, Clone, Serialize)]
pub struct Tidy {
    pub id: &'static str,
    pub title: &'static str,
    pub detail: &'static str,
    pub on: bool,
    pub available: bool,
    pub unavailable_why: Option<String>,
    pub last: Option<Last>,
}

fn on_key(id: Id) -> String {
    format!("tidy.on.{}", id.id())
}

fn last_key(id: Id) -> String {
    format!("tidy.last.{}", id.id())
}

pub fn is_on(store: &Store, id: Id) -> Result<bool> {
    Ok(store.meta(&on_key(id))?.as_deref() == Some("1"))
}

pub fn set_on(store: &Store, id: Id, on: bool) -> Result<()> {
    store.set_meta(&on_key(id), if on { "1" } else { "0" })
}

pub fn last(store: &Store, id: Id) -> Result<Option<Last>> {
    Ok(store
        .meta(&last_key(id))?
        .and_then(|v| serde_json::from_str(&v).ok()))
}

pub fn record(store: &Store, id: Id, last: &Last) -> Result<()> {
    store.set_meta(&last_key(id), &serde_json::to_string(last)?)
}

/// Every tidy-up, whether it's on, and what it did last.
pub fn list(store: &Store) -> Result<Vec<Tidy>> {
    Id::ALL
        .into_iter()
        .map(|id| {
            let why = id.unavailable();
            Ok(Tidy {
                id: id.id(),
                title: id.title(),
                detail: id.detail(),
                on: is_on(store, id)?,
                available: why.is_none(),
                unavailable_why: why,
                last: last(store, id)?,
            })
        })
        .collect()
}

/// The tidy-ups that are on and due at `now`: never run, or not for a day.
pub fn due(store: &Store, now: i64) -> Result<Vec<Id>> {
    let mut out = Vec::new();
    for id in Id::ALL {
        if !is_on(store, id)? {
            continue;
        }
        let ran = last(store, id)?.map(|l| l.at);
        // A clock set back counts as due, rather than as never.
        if ran.is_none_or(|at| now - at >= EVERY || at > now) {
            out.push(id);
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Done {
    pub id: &'static str,
    pub path: PathBuf,
    /// `deleted` or `emptied` (on a dry run: what would be).
    pub what: &'static str,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Failed {
    pub id: &'static str,
    /// Empty when it isn't about one thing (Xcode isn't there).
    pub path: PathBuf,
    pub why: String,
}

/// What one tidy-up did, or on a dry run would do.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub done: Vec<Done>,
    pub failed: Vec<Failed>,
    /// Left alone, and why: what you keep, and what isn't what it seemed
    /// when checked again. Not failures: the next run leaves them alone too.
    pub skipped: Vec<(PathBuf, String)>,
    /// One sentence; `None` until the run ends.
    pub note: Option<String>,
}

impl Outcome {
    pub fn freed(&self) -> u64 {
        self.done.iter().map(|d| d.bytes).sum()
    }

    pub fn last(&self, at: i64) -> Last {
        Last {
            at,
            freed: self.freed(),
            done: self.done.len(),
            failed: self.failed.len(),
            note: self.note.clone(),
        }
    }

    fn fail(&mut self, id: Id, path: impl Into<PathBuf>, why: impl Into<String>) {
        self.failed.push(Failed {
            id: id.id(),
            path: path.into(),
            why: why.into(),
        });
    }

    fn did(&mut self, id: Id, path: impl Into<PathBuf>, bytes: u64) {
        self.done.push(Done {
            id: id.id(),
            path: path.into(),
            what: id.verb(),
            bytes,
        });
    }

    /// The note: what was done (`what`, "Emptied 4 caches"), and what
    /// couldn't be.
    fn finish(&mut self, id: Id, what: Option<String>) {
        let verb = id.verb();
        let note = match (what, self.failed.len()) {
            (None, 0) => "Nothing to tidy".to_string(),
            (None, 1) => self.failed[0].why.trim_end_matches('.').to_string(),
            (None, n) => format!("{n} couldn't be {verb}"),
            (Some(w), 0) => w,
            (Some(w), n) => format!("{w}; {n} more couldn't be {verb}"),
        };
        self.note = Some(note);
    }
}

/// Runs a tidy-up now (`dry_run`: says what it would do, and does nothing).
pub fn run(id: Id, home: &Path, keep: &Keep, now: i64, dry_run: bool) -> Outcome {
    // Measuring must never download a cloud placeholder.
    if let Err(e) = mdif_macos::fs::forbid_dataless_materialization() {
        let mut o = Outcome::default();
        o.fail(
            id,
            PathBuf::new(),
            format!("Can't keep cloud files from downloading: {e}"),
        );
        o.finish(id, None);
        return o;
    }
    match id {
        Id::Simulators => simulators(home, keep, dry_run),
        Id::Builds | Id::Dependencies => unused(id, home, keep, now, dry_run, &privacy::may_open),
        Id::Incremental => incremental(home, keep, now, dry_run),
        Id::Caches => caches(home, keep, now, dry_run, CACHE_MIN, &privacy::may_open),
    }
}

/// The entry that keeps `path`, or something in it.
fn kept<'k>(keep: &'k Keep, path: &Path) -> Option<&'k str> {
    keep.covering(path).or_else(|| keep.inside(path))
}

fn kept_why(entry: &str) -> String {
    format!("you keep {}", crate::text::printable(entry))
}

// ---------------------------------------------------------------------------
// Simulators.

fn simulators(home: &Path, keep: &Keep, dry_run: bool) -> Outcome {
    let id = Id::Simulators;
    let mut o = Outcome::default();
    let Some(simctl) = simctl() else {
        o.fail(id, PathBuf::new(), NO_XCODE);
        o.finish(id, None);
        return o;
    };
    let sim = |args: &[String], wait: u64| {
        let mut c = Command::new(&simctl);
        c.args(args);
        run_within(c, Duration::from_secs(wait))
    };
    let failure = |out: Option<&std::process::Output>| match out {
        None => "simctl didn't answer.".to_string(),
        Some(out) => {
            let text = String::from_utf8_lossy(if out.stderr.is_empty() {
                &out.stdout
            } else {
                &out.stderr
            })
            .trim()
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
            format!("simctl: {}", crate::text::printable(&text))
        }
    };

    // Runtimes, as simctl picks them.
    let runtimes = sim(&strings(&["runtime", "list", "-j"]), 30)
        .filter(|out| out.status.success())
        .map(|out| parse_runtimes(&out.stdout))
        .unwrap_or_default();
    let mut names = Vec::new();
    match sim(&runtime_dry_run_args(), 60) {
        Some(out) if out.status.success() || nothing_to_delete(&out) => {
            for uuid in parse_would_delete(&String::from_utf8_lossy(&out.stdout)) {
                // Where it is tells whether you keep it: one simctl doesn't
                // list stays.
                let (Some(rt), Some(args)) = (
                    runtimes
                        .iter()
                        .find(|r| r.identifier == uuid && !r.path.as_os_str().is_empty()),
                    runtime_delete_args(&uuid),
                ) else {
                    continue;
                };
                let (name, path, bytes) = (rt.name.clone(), rt.path.clone(), rt.bytes);
                if let Some(entry) = kept(keep, &path) {
                    o.skipped.push((path, kept_why(entry)));
                    continue;
                }
                if dry_run {
                    o.did(id, path, bytes);
                    names.push(name);
                    continue;
                }
                match sim(&args, 600) {
                    Some(out) if out.status.success() => {
                        o.did(id, path, bytes);
                        names.push(name);
                    }
                    other => {
                        let why = format!("The {name} wasn't deleted: {}", failure(other.as_ref()));
                        o.fail(id, path, why);
                    }
                }
            }
        }
        other => o.fail(id, PathBuf::new(), failure(other.as_ref())),
    }

    // Simulators left without a runtime. On a dry run, those without one
    // now: the runtimes it would delete are still there.
    let devices = sim(&strings(&["list", "devices", "-j"]), 30)
        .filter(|out| out.status.success())
        .map(|out| parse_unavailable(&out.stdout))
        .unwrap_or_default();
    let mut gone = 0;
    for d in devices {
        let dir = d.dir.clone().unwrap_or_else(|| {
            home.join("Library/Developer/CoreSimulator/Devices")
                .join(&d.udid)
        });
        if let Some(entry) = kept(keep, &dir) {
            o.skipped.push((dir, kept_why(entry)));
            continue;
        }
        let Some(args) = device_delete_args(&d.udid) else {
            continue;
        };
        if dry_run {
            o.did(id, dir, d.bytes);
            gone += 1;
            continue;
        }
        match sim(&args, 120) {
            Some(out) if out.status.success() => {
                o.did(id, dir, d.bytes);
                gone += 1;
            }
            other => {
                let why = format!(
                    "The simulator {} wasn't deleted: {}",
                    crate::text::printable(&d.name),
                    failure(other.as_ref())
                );
                o.fail(id, dir, why);
            }
        }
    }

    let runtimes = match names.as_slice() {
        [] => None,
        [one] => Some(format!("the {one}")),
        many => Some(format!("{} simulator runtimes", many.len())),
    };
    let devices = match gone {
        0 => None,
        1 => Some("a simulator left without a runtime".to_string()),
        n => Some(format!("{n} simulators left without a runtime")),
    };
    let what = match (runtimes, devices) {
        (None, None) => None,
        (Some(r), None) => Some(format!("Deleted {r}")),
        (None, Some(d)) => Some(format!("Deleted {d}")),
        (Some(r), Some(d)) => Some(format!("Deleted {r} and {d}")),
    };
    o.finish(id, what);
    o
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

/// `simctl runtime delete --notUsedSinceDays 60 --dry-run`: what simctl
/// would delete, deleted then one by one, so that a runtime you keep stays.
pub fn runtime_dry_run_args() -> Vec<String> {
    vec![
        "runtime".into(),
        "delete".into(),
        "--notUsedSinceDays".into(),
        SIMULATOR_DAYS.to_string(),
        "--dry-run".into(),
    ]
}

/// `simctl runtime delete <uuid>`; nothing for what isn't a UUID (simctl
/// takes `all` too).
pub fn runtime_delete_args(uuid: &str) -> Option<Vec<String>> {
    is_uuid(uuid).then(|| strings(&["runtime", "delete", uuid]))
}

/// `simctl delete <udid>`; nothing for what isn't a UUID (simctl takes
/// `all` and `unavailable` too).
pub fn device_delete_args(udid: &str) -> Option<Vec<String>> {
    is_uuid(udid).then(|| strings(&["delete", udid]))
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// simctl's dry run with nothing to delete ends in failure, saying so.
fn nothing_to_delete(out: &std::process::Output) -> bool {
    let said = |b: &[u8]| String::from_utf8_lossy(b).contains("No matching images");
    said(&out.stdout) || said(&out.stderr)
}

/// The runtimes a dry run would delete: "Would delete P: 2BFB81AB-… iOS
/// (27.0 - 24A434) (Ready)".
pub fn parse_would_delete(out: &str) -> Vec<String> {
    out.lines()
        .filter(|l| l.trim_start().starts_with("Would delete"))
        .filter_map(|l| l.split_whitespace().find(|w| is_uuid(w)))
        .map(str::to_string)
        .collect()
}

/// A runtime image, as `simctl runtime list -j` lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct SimRuntime {
    pub identifier: String,
    /// "iOS 27.0 Simulator".
    pub name: String,
    /// The asset folder that holds it (as the Overview shows it), or the
    /// image itself.
    pub path: PathBuf,
    pub bytes: u64,
}

pub fn parse_runtimes(out: &[u8]) -> Vec<SimRuntime> {
    let Ok(serde_json::Value::Object(all)) = serde_json::from_slice(out) else {
        return Vec::new();
    };
    all.iter()
        .map(|(key, r)| {
            let image = r["path"]
                .as_str()
                .or(r["parentMountPath"].as_str())
                .unwrap_or("");
            let path = match image.find(".asset/") {
                Some(at) => &image[..at + ".asset".len()],
                None => image,
            };
            SimRuntime {
                identifier: r["identifier"].as_str().unwrap_or(key).to_string(),
                name: runtime_name(r),
                path: PathBuf::from(path),
                bytes: r["sizeBytes"].as_u64().unwrap_or(0),
            }
        })
        .collect()
}

/// A simulator whose runtime is gone.
#[derive(Debug, Clone, PartialEq)]
pub struct SimDevice {
    pub udid: String,
    pub name: String,
    /// Its folder, which deleting it deletes.
    pub dir: Option<PathBuf>,
    pub bytes: u64,
}

/// The simulators `simctl list devices -j` says aren't available.
pub fn parse_unavailable(out: &[u8]) -> Vec<SimDevice> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(out) else {
        return Vec::new();
    };
    let Some(by_runtime) = v["devices"].as_object() else {
        return Vec::new();
    };
    by_runtime
        .values()
        .filter_map(|list| list.as_array())
        .flatten()
        .filter(|d| {
            d["isAvailable"].as_bool() == Some(false)
                // Before Xcode 10.1: "(unavailable, runtime profile not found)".
                || d["availability"]
                    .as_str()
                    .is_some_and(|a| a.contains("unavailable"))
        })
        .filter_map(|d| {
            Some(SimDevice {
                udid: d["udid"].as_str()?.to_string(),
                name: d["name"].as_str().unwrap_or("").to_string(),
                dir: d["dataPath"]
                    .as_str()
                    .and_then(|p| Path::new(p).parent())
                    .map(Path::to_path_buf),
                bytes: d["dataPathSize"].as_u64().unwrap_or(0),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Old build output and node_modules.

/// Build output (`Builds`) or `node_modules` (`Dependencies`) nothing has
/// written to or read from for [`BUILD_AGE`].
fn unused(
    id: Id,
    home: &Path,
    keep: &Keep,
    now: i64,
    dry_run: bool,
    may_open: &dyn Fn(&Path) -> bool,
) -> Outcome {
    let mut o = Outcome::default();
    let mut projects: Vec<PathBuf> = Vec::new();
    let found = match id {
        Id::Dependencies => dependency_dirs(home),
        _ => build_dirs(home),
    };
    for dir in found {
        if let Some(entry) = kept(keep, &dir) {
            o.skipped.push((dir, kept_why(entry)));
            continue;
        }
        // Cheap first: a build or an install near the top.
        if newest(&dir, 2, may_open).is_none_or(|t| now - t < BUILD_AGE) {
            continue;
        }
        // Then all of it: nothing written anywhere in it, and nothing in it
        // run or read (a tool used from `target/release` stays, and so do
        // the packages of a project that runs).
        if usage(&dir, may_open).is_none_or(|u| now - u.newest < BUILD_AGE) {
            continue;
        }
        let step = match clean::plan(&dir, home, keep, &[], &[]) {
            Ok(s @ Step::Delete(_)) => s,
            Ok(_) => continue,
            Err(why) => {
                o.skipped.push((dir, why));
                continue;
            }
        };
        // Hard links (Rust's `deps`, pnpm's store) count once.
        let bytes = crate::scan::size_of(&dir).unwrap_or(0);
        let project = dir.parent().map(Path::to_path_buf);
        if dry_run {
            o.did(id, dir, bytes);
        } else {
            match clean::run(&step, home) {
                Ok(_) => o.did(id, dir, bytes),
                Err(why) => {
                    o.fail(id, dir, why);
                    continue;
                }
            }
        }
        if let Some(p) = project.filter(|p| !projects.contains(p)) {
            projects.push(p);
        }
    }
    let what = match (id, projects.as_slice()) {
        (_, []) => None,
        (Id::Dependencies, [one]) => Some(format!(
            "Deleted the node_modules of {}",
            display(one, home)
        )),
        (Id::Dependencies, many) => Some(format!(
            "Deleted the node_modules of {} projects",
            many.len()
        )),
        (_, [one]) => Some(format!("Deleted the old build of {}", display(one, home))),
        (_, many) => Some(format!("Deleted the old builds of {} projects", many.len())),
    };
    o.finish(id, what);
    o
}

/// The projects' build output ([`clean::build_output`]) in the code
/// folders, agents' worktrees included.
pub fn build_dirs(home: &Path) -> Vec<PathBuf> {
    artifact_dirs(home, clean::build_output)
}

/// The projects' `node_modules` ([`clean::dependencies`]) in the code
/// folders, agents' worktrees included.
pub fn dependency_dirs(home: &Path) -> Vec<PathBuf> {
    artifact_dirs(home, clean::dependencies)
}

/// The folders named in [`ARTIFACTS`] that `take` takes, in the code
/// folders. Never into one of those, `.git` or other hidden folders (but
/// `.claude/worktrees`).
fn artifact_dirs(home: &Path, take: fn(&Path) -> bool) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, take: fn(&Path) -> bool, out: &mut Vec<PathBuf>) {
        for (name, path) in subdirs(dir) {
            let Some(name) = name.to_str() else {
                continue;
            };
            if !privacy::may_open(&path) {
                continue;
            }
            if ARTIFACTS.contains(&name) {
                if take(&path) {
                    out.push(path);
                }
                continue;
            }
            match name {
                ".claude" => {
                    let worktrees = path.join("worktrees");
                    if depth + 2 <= PROJECT_DEPTH && real_dir(&worktrees) {
                        walk(&worktrees, depth + 2, take, out);
                    }
                }
                n if n.starts_with('.') => {}
                _ if depth < PROJECT_DEPTH => walk(&path, depth + 1, take, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for root in CODE_ROOTS {
        let root = home.join(root);
        if privacy::may_open(&root) && real_dir(&root) {
            walk(&root, 0, take, &mut out);
        }
    }
    out.sort();
    out.dedup();
    out
}

// ---------------------------------------------------------------------------
// A Rust build's incremental caches.

fn incremental(home: &Path, keep: &Keep, now: i64, dry_run: bool) -> Outcome {
    let id = Id::Incremental;
    let mut o = Outcome::default();
    let mut projects: Vec<PathBuf> = Vec::new();
    for dir in incremental_dirs(home) {
        if let Some(entry) = kept(keep, &dir) {
            o.skipped.push((dir, kept_why(entry)));
            continue;
        }
        if newest(&dir, 2, &privacy::may_open).is_none_or(|t| now - t < INCREMENTAL_AGE) {
            continue;
        }
        let step = match clean::plan(&dir, home, keep, &[], &[]) {
            Ok(s @ Step::Delete(_)) => s,
            Ok(_) => continue,
            Err(why) => {
                o.skipped.push((dir, why));
                continue;
            }
        };
        let bytes = crate::scan::size_of(&dir).unwrap_or(0);
        let project = project_of(&dir);
        if dry_run {
            o.did(id, dir, bytes);
        } else {
            match clean::run(&step, home) {
                Ok(_) => o.did(id, dir, bytes),
                Err(why) => {
                    o.fail(id, dir, why);
                    continue;
                }
            }
        }
        if let Some(p) = project.filter(|p| !projects.contains(p)) {
            projects.push(p);
        }
    }
    let what = match projects.as_slice() {
        [] => None,
        [one] => Some(format!(
            "Deleted the old incremental caches in {}",
            display(one, home)
        )),
        many => Some(format!(
            "Deleted the old incremental caches of {} Rust projects",
            many.len()
        )),
    };
    o.finish(id, what);
    o
}

/// The project whose build `incremental` is in: the folder `target` is in.
fn project_of(incremental: &Path) -> Option<PathBuf> {
    incremental
        .ancestors()
        .skip(2)
        .take(2)
        .find(|a| a.file_name().is_some_and(|n| n == "target"))
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

/// A Rust build's incremental caches (`target/<profile>/incremental`,
/// `target/<triple>/<profile>/incremental`) in the projects in the code
/// folders, agents' worktrees included. Pruned: never into `.git`,
/// `node_modules`, other build output, hidden folders (but
/// `.claude/worktrees`), or `target` beyond those caches.
pub fn incremental_dirs(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in CODE_ROOTS {
        let root = home.join(root);
        if privacy::may_open(&root) && real_dir(&root) {
            projects(&root, 0, &mut out);
        }
    }
    out.sort();
    out.dedup();
    out
}

fn projects(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    for (name, path) in subdirs(dir) {
        let Some(name) = name.to_str() else {
            continue;
        };
        if !privacy::may_open(&path) {
            continue;
        }
        match name {
            // Any other `target` is pruned with the build output below.
            "target" if dir.join("Cargo.toml").is_file() => out.extend(incremental_in(&path)),
            ".claude" => {
                let worktrees = path.join("worktrees");
                if depth + 2 <= PROJECT_DEPTH && real_dir(&worktrees) {
                    projects(&worktrees, depth + 2, out);
                }
            }
            n if n.starts_with('.') || ARTIFACTS.contains(&n) => {}
            _ if depth < PROJECT_DEPTH => projects(&path, depth + 1, out),
            _ => {}
        }
    }
}

/// The incremental caches in a `target`: in a profile's folder, or a
/// target triple's profile's.
fn incremental_in(target: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for (_, sub) in subdirs(target) {
        let inc = sub.join("incremental");
        if real_dir(&inc) {
            out.push(inc);
            continue;
        }
        for (_, profile) in subdirs(&sub) {
            let inc = profile.join("incremental");
            if real_dir(&inc) {
                out.push(inc);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Caches.

fn caches(
    home: &Path,
    keep: &Keep,
    now: i64,
    dry_run: bool,
    min: u64,
    may_open: &dyn Fn(&Path) -> bool,
) -> Outcome {
    let id = Id::Caches;
    let mut o = Outcome::default();
    let mut owners: Vec<String> = Vec::new();
    for dir in cache_folders(home, may_open) {
        let Some(owner) = clean::cache_of(&dir, home) else {
            continue;
        };
        if NOT_TIDIED.contains(&owner.as_str()) {
            continue;
        }
        if let Some(entry) = kept(keep, &dir) {
            o.skipped.push((dir, kept_why(entry)));
            continue;
        }
        // Cheap first: something written near the top.
        if newest(&dir, 2, may_open).is_none_or(|t| now - t < CACHE_AGE) {
            continue;
        }
        // Then all of it: nothing written or read anywhere in it.
        let Some(before) = usage(&dir, may_open) else {
            continue;
        };
        if before.bytes < min || now - before.newest < CACHE_AGE {
            continue;
        }
        let step = match clean::plan(&dir, home, keep, &[], &[]) {
            Ok(s @ Step::Empty { .. }) => s,
            Ok(_) => continue,
            Err(why) => {
                o.skipped.push((dir, why));
                continue;
            }
        };
        if dry_run {
            o.did(id, dir, before.bytes);
        } else {
            match clean::run(&step, home) {
                Ok(_) => {
                    // What couldn't be deleted (a file only its app may
                    // touch) stays, and isn't counted.
                    let after = usage(&dir, may_open).map_or(0, |u| u.bytes);
                    o.did(id, dir, before.bytes.saturating_sub(after));
                }
                Err(why) => {
                    o.fail(id, dir, why);
                    continue;
                }
            }
        }
        owners.push(owner);
    }
    let what = match owners.as_slice() {
        [] => None,
        [one] => Some(format!("Emptied {}'s cache", crate::text::printable(one))),
        many => Some(format!("Emptied {} caches", many.len())),
    };
    o.finish(id, what);
    o
}

/// The cache folders `clean` empties (see [`clean::cache_of`]): in
/// `~/Library/Caches`, apps' containers, `~/Library/Application Support`
/// and `~/.cache`. Nothing `may_open` refuses is looked at, not even to see
/// whether it's there: another app's container shows a privacy prompt.
fn cache_folders(home: &Path, may_open: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let children = |dir: &Path| -> Vec<PathBuf> {
        if !may_open(dir) {
            return Vec::new();
        }
        subdirs(dir).into_iter().map(|(_, p)| p).collect()
    };
    let mut out = children(&home.join("Library/Caches"));
    out.extend(children(&home.join(".cache")));
    let inside = |dir: PathBuf| (may_open(&dir) && real_dir(&dir)).then_some(dir);
    out.extend(
        children(&home.join("Library/Containers"))
            .into_iter()
            .filter_map(|c| inside(c.join("Data/Library/Caches"))),
    );
    for app in children(&home.join("Library/Application Support")) {
        out.extend(
            ["Cache", "Caches", "Code Cache", "GPUCache"]
                .into_iter()
                .filter_map(|n| inside(app.join(n))),
        );
    }
    out.retain(|p| clean::cache_of(p, home).is_some());
    out
}

// ---------------------------------------------------------------------------
// Looking, metadata only.

/// The real folders in `dir` (links aren't followed), with their names.
fn subdirs(dir: &Path) -> Vec<(std::ffi::OsString, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| (e.file_name(), e.path()))
        .collect()
}

fn real_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// A folder whose contents aren't on this disk (a cloud placeholder):
/// listing it would download them.
fn dataless(m: &std::fs::Metadata) -> bool {
    m.st_flags() & mdif_macos::bulk::SF_DATALESS != 0
}

/// The newest modification time of `dir` and what's in it, `depth` levels
/// down; `None` if it isn't a folder that may be opened.
fn newest(dir: &Path, depth: usize, may_open: &dyn Fn(&Path) -> bool) -> Option<i64> {
    if !may_open(dir) {
        return None;
    }
    let m = std::fs::symlink_metadata(dir).ok()?;
    if !m.is_dir() {
        return None;
    }
    fn down(dir: &Path, depth: usize, dev: u64, newest: &mut i64) {
        if depth == 0 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            // Not followed: the link's own times.
            let Ok(m) = e.metadata() else {
                continue;
            };
            if m.dev() != dev {
                continue;
            }
            *newest = (*newest).max(m.mtime());
            if m.is_dir() && !dataless(&m) {
                down(&e.path(), depth - 1, dev, newest);
            }
        }
    }
    let mut t = m.mtime();
    down(dir, depth, m.dev(), &mut t);
    Some(t)
}

/// All of a folder, measured.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Usage {
    /// Allocated bytes (a hard-linked file counts once per link).
    bytes: u64,
    /// When something in it was last written, or a file in it read.
    newest: i64,
}

/// `dir` and everything in it, on its volume, links not followed, cloud
/// placeholders not entered. Access times say when a file was last read; a
/// folder's say when it was last listed (by this, too), so only files'
/// count.
fn usage(dir: &Path, may_open: &dyn Fn(&Path) -> bool) -> Option<Usage> {
    if !may_open(dir) {
        return None;
    }
    let root = std::fs::symlink_metadata(dir).ok()?;
    if !root.is_dir() {
        return None;
    }
    let dev = root.dev();
    let mut u = Usage {
        bytes: root.blocks() * 512,
        newest: root.mtime(),
    };
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else {
                continue;
            };
            if m.dev() != dev {
                continue;
            }
            u.bytes += m.blocks() * 512;
            u.newest = u.newest.max(m.mtime());
            if m.is_dir() {
                if !dataless(&m) {
                    todo.push(e.path());
                }
            } else if m.is_file() {
                u.newest = u.newest.max(m.atime());
            }
        }
    }
    Some(u)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, FileTimes};
    use std::time::{SystemTime, UNIX_EPOCH};

    const NOW: i64 = 1_792_600_000;

    fn at(ts: i64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(ts as u64)
    }

    /// Sets the times of `path` and everything in it, deepest first (a
    /// folder's modification time changes when something is added to it).
    fn age(path: &Path, ts: i64) {
        let m = std::fs::symlink_metadata(path).unwrap();
        if m.is_dir() {
            for e in std::fs::read_dir(path).unwrap().flatten() {
                age(&e.path(), ts);
            }
        }
        let times = FileTimes::new().set_accessed(at(ts)).set_modified(at(ts));
        File::open(path).unwrap().set_times(times).unwrap();
    }

    fn write(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![7u8; bytes]).unwrap();
    }

    /// A Rust project with a debug build's incremental caches.
    fn rust_project(home: &Path, rel: &str) -> PathBuf {
        let p = home.join(rel);
        write(&p.join("Cargo.toml"), 10);
        let inc = p.join("target/debug/incremental");
        write(&inc.join("app-1x2y/s-abc-def/query-cache.bin"), 20_000);
        write(&p.join("target/debug/app"), 10_000);
        inc
    }

    #[test]
    fn incremental_caches_are_found_in_projects_and_worktrees_only() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let app = rust_project(h, "devel/app");
        let tauri = rust_project(h, "devel/web/src-tauri");
        let wt = rust_project(h, "devel/app/.claude/worktrees/feature");
        let cross = h.join("devel/app/target/aarch64-apple-darwin/release/incremental");
        write(&cross.join("app-9z/s-1/dep-graph.bin"), 1000);
        // Not where a Rust project's build is, or pruned.
        rust_project(h, "devel/web/node_modules/thing");
        rust_project(h, "devel/.hidden/app");
        rust_project(h, "devel/app/.git/app");
        rust_project(h, "devel/a/b/c/d/e/f/g/app");
        rust_project(h, "Elsewhere/app");
        write(&h.join("devel/maven/target/debug/incremental/x"), 10);
        // A link to a project isn't followed.
        std::os::unix::fs::symlink(h.join("devel/app"), h.join("devel/link")).unwrap();

        let mut want = vec![app, cross, tauri, wt];
        want.sort();
        assert_eq!(incremental_dirs(h), want);
    }

    #[test]
    fn incremental_caches_go_once_no_build_touched_them_for_30_days() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let old = rust_project(h, "devel/old");
        let fresh = rust_project(h, "devel/fresh");
        let deep = rust_project(h, "devel/deep");
        let kept = rust_project(h, "devel/kept");
        for p in ["devel/old", "devel/fresh", "devel/deep", "devel/kept"] {
            age(&h.join(p), NOW - 40 * DAY);
        }
        // A build 3 days ago made a new session in one crate's folder.
        age(&fresh.join("app-1x2y/s-abc-def"), NOW - 3 * DAY);
        // Deeper than two levels doesn't count: a session's own files.
        age(
            &deep.join("app-1x2y/s-abc-def/query-cache.bin"),
            NOW - 3 * DAY,
        );
        let keep = Keep::parse("~/devel/kept/target\n", h);

        let o = run(Id::Incremental, h, &keep, NOW, true);
        let paths: Vec<&Path> = o.done.iter().map(|d| d.path.as_path()).collect();
        assert_eq!(paths, [deep.as_path(), old.as_path()]);
        assert!(o.done.iter().all(|d| d.what == "deleted" && d.bytes > 0));
        assert_eq!(o.skipped.len(), 1);
        assert_eq!(o.skipped[0].0, kept);
        assert!(o.skipped[0].1.contains("you keep ~/devel/kept/target"));
        assert!(o.failed.is_empty());
        // A dry run deletes nothing.
        assert!(old.is_dir() && deep.is_dir());

        let o = run(Id::Incremental, h, &keep, NOW, false);
        assert_eq!(o.done.len(), 2);
        assert!(!old.exists() && !deep.exists());
        assert!(h.join("devel/old/target/debug/app").is_file());
        assert!(fresh.is_dir() && kept.is_dir());
        assert_eq!(
            o.note.as_deref(),
            Some("Deleted the old incremental caches of 2 Rust projects")
        );
        let last = o.last(NOW);
        assert_eq!((last.done, last.failed), (2, 0));
        assert_eq!(last.freed, o.freed());

        let again = run(Id::Incremental, h, &keep, NOW, false);
        assert!(again.done.is_empty());
        assert_eq!(again.note.as_deref(), Some("Nothing to tidy"));
    }

    #[test]
    fn build_output_is_found_in_projects_and_worktrees_only() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let rust = rust_project(h, "devel/app")
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf();
        let wt = rust_project(h, "devel/app/.claude/worktrees/feature")
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf();
        write(&h.join("devel/flutter/pubspec.yaml"), 10);
        write(&h.join("devel/flutter/build/app/outputs/app.apk"), 100);
        write(&h.join("devel/flutter/.dart_tool/x"), 10);
        write(&h.join("devel/swift/Package.swift"), 10);
        write(&h.join("devel/swift/.build/debug/x"), 10);
        // Not build output: no file its toolchain builds from, what comes
        // back only by downloading it, hidden or too deep, or elsewhere.
        write(&h.join("devel/py/build/lib/x.py"), 10);
        write(&h.join("devel/web/package.json"), 10);
        write(&h.join("devel/web/node_modules/thing/x.js"), 10);
        rust_project(h, "devel/web/node_modules/thing");
        rust_project(h, "devel/.hidden/app");
        rust_project(h, "devel/a/b/c/d/e/f/g/app");
        rust_project(h, "Elsewhere/app");
        std::os::unix::fs::symlink(h.join("devel/app"), h.join("devel/link")).unwrap();

        let mut want = vec![
            rust,
            wt,
            h.join("devel/flutter/.dart_tool"),
            h.join("devel/flutter/build"),
            h.join("devel/swift/.build"),
        ];
        want.sort();
        assert_eq!(build_dirs(h), want);
    }

    #[test]
    fn build_output_goes_once_nothing_built_into_or_ran_from_it_for_14_days() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let target = |p: &str| h.join(p).join("target");
        for p in ["devel/old", "devel/built", "devel/ran", "devel/kept"] {
            rust_project(h, p);
            age(&h.join(p), NOW - 20 * DAY);
        }
        // A build 3 days ago, deep in it; a binary in it run yesterday.
        age(
            &target("devel/built").join("debug/incremental/app-1x2y/s-abc-def/query-cache.bin"),
            NOW - 3 * DAY,
        );
        File::open(target("devel/ran").join("debug/app"))
            .unwrap()
            .set_times(FileTimes::new().set_accessed(at(NOW - DAY)))
            .unwrap();
        let keep = Keep::parse("~/devel/kept/target\n", h);

        let o = run(Id::Builds, h, &keep, NOW, true);
        let paths: Vec<PathBuf> = o.done.iter().map(|d| d.path.clone()).collect();
        assert_eq!(paths, [target("devel/old")]);
        assert!(o.done[0].bytes > 0 && o.done[0].what == "deleted");
        assert_eq!(o.skipped.len(), 1);
        assert_eq!(o.skipped[0].0, target("devel/kept"));
        assert!(target("devel/old").is_dir(), "a dry run deletes nothing");

        let o = run(Id::Builds, h, &keep, NOW, false);
        assert_eq!(o.done.len(), 1);
        assert!(!target("devel/old").exists());
        assert!(h.join("devel/old/Cargo.toml").is_file());
        assert!(target("devel/built").is_dir() && target("devel/ran").is_dir());
        assert!(target("devel/kept").is_dir());
        assert_eq!(
            o.note.as_deref(),
            Some("Deleted the old build of ~/devel/old")
        );
    }

    #[test]
    fn node_modules_goes_once_nothing_installed_into_or_ran_from_it_for_14_days() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let modules = |p: &str| h.join(p).join("node_modules");
        for p in [
            "devel/old",
            "devel/ran",
            "devel/nolock",
            "devel/wt/.claude/worktrees/a",
        ] {
            write(&h.join(p).join("package.json"), 10);
            if p != "devel/nolock" {
                write(&h.join(p).join("package-lock.json"), 10);
            }
            write(&modules(p).join("left-pad/index.js"), 5_000);
            write(&modules(p).join("left-pad/package.json"), 100);
            age(&h.join(p), NOW - 20 * DAY);
        }
        // A project that ran yesterday read its packages.
        File::open(modules("devel/ran").join("left-pad/index.js"))
            .unwrap()
            .set_times(FileTimes::new().set_accessed(at(NOW - DAY)))
            .unwrap();

        let mut want = vec![
            modules("devel/old"),
            modules("devel/ran"),
            modules("devel/wt/.claude/worktrees/a"),
        ];
        want.sort();
        assert_eq!(dependency_dirs(h), want, "only with a lockfile");

        let o = run(Id::Dependencies, h, &Keep::default(), NOW, false);
        let mut gone: Vec<PathBuf> = o.done.iter().map(|d| d.path.clone()).collect();
        gone.sort();
        assert_eq!(
            gone,
            [
                modules("devel/old"),
                modules("devel/wt/.claude/worktrees/a")
            ]
        );
        assert!(!modules("devel/old").exists() && h.join("devel/old/package-lock.json").is_file());
        assert!(modules("devel/ran").is_dir() && modules("devel/nolock").is_dir());
        assert_eq!(
            o.note.as_deref(),
            Some("Deleted the node_modules of 2 projects")
        );
    }

    /// An old cache: `bytes` in a file three folders down.
    fn cache(home: &Path, rel: &str, bytes: usize, ts: i64) -> PathBuf {
        let dir = home.join(rel);
        write(&dir.join("a/b/c/blob"), bytes);
        write(&dir.join("index"), 100);
        age(&dir, ts);
        dir
    }

    #[test]
    fn caches_nothing_used_for_30_days_are_emptied() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let old = NOW - 40 * DAY;
        let big = cache(h, "Library/Caches/com.example.mdif-old", 300_000, old);
        let tool = cache(h, ".cache/mdif-test-tool", 300_000, old);
        let electron = cache(
            h,
            "Library/Application Support/MdifChat/Code Cache",
            300_000,
            old,
        );
        let small = cache(h, "Library/Caches/com.example.mdif-small", 1_000, old);
        let written = cache(h, "Library/Caches/com.example.mdif-written", 300_000, old);
        let read = cache(h, "Library/Caches/com.example.mdif-read", 300_000, old);
        let apple = cache(h, "Library/Caches/com.apple.mdif-test", 300_000, old);
        let models = cache(h, ".cache/huggingface", 300_000, old);
        let kept = cache(h, "Library/Caches/com.example.mdif-kept", 300_000, old);
        // Written to deep inside 2 days ago, or read yesterday.
        let blob = written.join("a/b/c/blob");
        File::open(&blob)
            .unwrap()
            .set_times(FileTimes::new().set_modified(at(NOW - 2 * DAY)))
            .unwrap();
        File::open(read.join("a/b/c/blob"))
            .unwrap()
            .set_times(FileTimes::new().set_accessed(at(NOW - DAY)))
            .unwrap();
        let keep = Keep::parse("~/Library/Caches/com.example.mdif-kept\n", h);
        let all = |_: &Path| true;

        let o = caches(h, &keep, NOW, true, 200_000, &all);
        let mut paths: Vec<PathBuf> = o.done.iter().map(|d| d.path.clone()).collect();
        paths.sort();
        let mut want = vec![big.clone(), tool.clone(), electron.clone()];
        want.sort();
        assert_eq!(paths, want);
        assert!(
            o.done
                .iter()
                .all(|d| d.what == "emptied" && d.bytes >= 300_000)
        );
        assert_eq!(
            o.skipped,
            [(
                kept.clone(),
                "you keep ~/Library/Caches/com.example.mdif-kept".to_string()
            )]
        );
        assert!(
            big.join("a/b/c/blob").is_file(),
            "a dry run empties nothing"
        );

        let o = caches(h, &keep, NOW, false, 200_000, &all);
        assert_eq!(o.done.len(), 3, "{:?}", o.failed);
        for d in [&big, &tool, &electron] {
            assert!(d.is_dir() && std::fs::read_dir(d).unwrap().count() == 0);
        }
        for d in [&small, &written, &read, &apple, &models, &kept] {
            assert!(d.join("a/b/c/blob").is_file(), "{}", d.display());
        }
        assert_eq!(o.note.as_deref(), Some("Emptied 3 caches"));
        assert!(o.freed() >= 900_000);
    }

    #[test]
    fn caches_behind_a_privacy_prompt_are_never_looked_at() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let old = NOW - 40 * DAY;
        let theirs = cache(
            h,
            "Library/Containers/com.example.mdif-app/Data/Library/Caches",
            300_000,
            old,
        );
        let mine = cache(h, "Library/Caches/com.example.mdif-mine", 300_000, old);
        let asked = std::cell::RefCell::new(Vec::new());
        let no_containers = |p: &Path| {
            asked.borrow_mut().push(p.to_path_buf());
            !p.starts_with(h.join("Library/Containers/com.example.mdif-app"))
        };
        assert_eq!(
            cache_folders(h, &no_containers),
            std::slice::from_ref(&mine)
        );
        assert!(asked.borrow().contains(&theirs), "asked before looking");
        let o = caches(h, &Keep::default(), NOW, true, 200_000, &no_containers);
        assert_eq!(o.done.len(), 1);
        assert_eq!(o.done[0].path, mine);
        // With Full Disk Access, it's a cache like the others.
        let mut both = cache_folders(h, &|_: &Path| true);
        both.sort();
        let mut want = vec![theirs, mine];
        want.sort();
        assert_eq!(both, want);
    }

    #[test]
    fn switches_and_last_runs_are_kept_in_the_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("history.sqlite3")).unwrap();
        assert!(due(&store, NOW).unwrap().is_empty(), "all off at first");
        assert!(!is_on(&store, Id::Caches).unwrap());
        set_on(&store, Id::Caches, true).unwrap();
        set_on(&store, Id::Incremental, true).unwrap();
        set_on(&store, Id::Incremental, false).unwrap();
        assert!(is_on(&store, Id::Caches).unwrap());
        assert!(!is_on(&store, Id::Incremental).unwrap());
        assert_eq!(due(&store, NOW).unwrap(), [Id::Caches]);

        let last = Last {
            at: NOW,
            freed: 123,
            done: 2,
            failed: 1,
            note: Some("Emptied 2 caches; 1 more couldn't be emptied".into()),
        };
        record(&store, Id::Caches, &last).unwrap();
        assert_eq!(super::last(&store, Id::Caches).unwrap(), Some(last));
        assert_eq!(super::last(&store, Id::Simulators).unwrap(), None);
        assert!(due(&store, NOW + EVERY - 1).unwrap().is_empty());
        assert_eq!(due(&store, NOW + EVERY).unwrap(), [Id::Caches]);
        assert_eq!(
            due(&store, NOW - 60).unwrap(),
            [Id::Caches],
            "clock set back"
        );

        let listed = list(&store).unwrap();
        let ids: Vec<&str> = listed.iter().map(|t| t.id).collect();
        assert_eq!(
            ids,
            [
                "simulators",
                "builds",
                "node-modules",
                "incremental",
                "caches"
            ]
        );
        assert!(listed[4].on && listed[4].last.is_some());
        assert!(!listed[3].on && listed[3].last.is_none());
        assert!(
            listed
                .iter()
                .all(|t| t.available == t.unavailable_why.is_none())
        );
        assert!(Id::ALL.iter().all(|&id| Id::parse(id.id()) == Some(id)));
        assert_eq!(Id::parse("everything"), None);
    }

    #[test]
    fn notes_say_what_was_done_and_what_was_not() {
        let mut o = Outcome::default();
        o.finish(Id::Caches, None);
        assert_eq!(o.note.as_deref(), Some("Nothing to tidy"));
        o.fail(
            Id::Caches,
            "/x",
            "Quit Example first: it's using ~/Library/Caches/x.",
        );
        o.finish(Id::Caches, None);
        assert_eq!(
            o.note.as_deref(),
            Some("Quit Example first: it's using ~/Library/Caches/x")
        );
        o.fail(Id::Caches, "/y", "Something else.");
        o.finish(Id::Caches, None);
        assert_eq!(o.note.as_deref(), Some("2 couldn't be emptied"));
        o.finish(Id::Caches, Some("Emptied 4 caches".into()));
        assert_eq!(
            o.note.as_deref(),
            Some("Emptied 4 caches; 2 more couldn't be emptied")
        );
    }

    #[test]
    fn simctl_is_asked_for_uuids_only() {
        assert_eq!(
            runtime_dry_run_args(),
            ["runtime", "delete", "--notUsedSinceDays", "60", "--dry-run"]
        );
        let uuid = "2BFB81AB-BA92-4510-90B5-F1DA4A8F84FC";
        assert_eq!(
            runtime_delete_args(uuid).unwrap(),
            ["runtime", "delete", uuid]
        );
        assert_eq!(device_delete_args(uuid).unwrap(), ["delete", uuid]);
        for not in [
            "all",
            "unavailable",
            "--notUsedSinceDays",
            "",
            "2BFB81AB-BA92-4510-90B5-F1DA4A8F84F",
        ] {
            assert_eq!(runtime_delete_args(not), None, "{not}");
            assert_eq!(device_delete_args(not), None, "{not}");
        }
        assert!(!is_uuid("2BFB81AB+BA92-4510-90B5-F1DA4A8F84FC"));
        assert!(!is_uuid("2BFB81AB-BA92-4510-90B5-F1DA4A8F84FX"));
    }

    #[test]
    fn simctl_output_is_read() {
        let dry = "Would delete P: 2BFB81AB-BA92-4510-90B5-F1DA4A8F84FC iOS (27.0 - 24A434) (Ready)\n\
                   Would delete D: 0A1B2C3D-0000-4000-8000-00000000000A watchOS (11.0 - 22R1) (Ready)\n\
                   Something else 11111111-2222-3333-4444-555555555555\n";
        assert_eq!(
            parse_would_delete(dry),
            [
                "2BFB81AB-BA92-4510-90B5-F1DA4A8F84FC",
                "0A1B2C3D-0000-4000-8000-00000000000A"
            ]
        );
        assert!(parse_would_delete("No matching images found to delete\n").is_empty());

        let runtimes = br#"{
          "2BFB81AB-BA92-4510-90B5-F1DA4A8F84FC" : {
            "identifier" : "2BFB81AB-BA92-4510-90B5-F1DA4A8F84FC",
            "path" : "/System/Library/AssetsV2/com_apple_MobileAsset_iOSSimulatorRuntime/3e80.asset/AssetData/Restore/043.dmg",
            "runtimeIdentifier" : "com.apple.CoreSimulator.SimRuntime.iOS-26-0",
            "sizeBytes" : 8067000161,
            "version" : "26.0"
          },
          "0A1B2C3D-0000-4000-8000-00000000000A" : {
            "path" : "/Library/Developer/CoreSimulator/Images/0A1B2C3D.dmg",
            "runtimeIdentifier" : "com.apple.CoreSimulator.SimRuntime.watchOS-11-0",
            "version" : "11.0"
          }
        }"#;
        let mut rt = parse_runtimes(runtimes);
        rt.sort_by(|a, b| a.identifier.cmp(&b.identifier));
        assert_eq!(
            rt,
            [
                SimRuntime {
                    identifier: "0A1B2C3D-0000-4000-8000-00000000000A".into(),
                    name: "watchOS 11.0 Simulator".into(),
                    path: "/Library/Developer/CoreSimulator/Images/0A1B2C3D.dmg".into(),
                    bytes: 0,
                },
                SimRuntime {
                    identifier: "2BFB81AB-BA92-4510-90B5-F1DA4A8F84FC".into(),
                    name: "iOS 26.0 Simulator".into(),
                    path: "/System/Library/AssetsV2/com_apple_MobileAsset_iOSSimulatorRuntime/3e80.asset".into(),
                    bytes: 8_067_000_161,
                },
            ]
        );
        assert!(parse_runtimes(b"not json").is_empty());

        let devices = br#"{
          "devices" : {
            "com.apple.CoreSimulator.SimRuntime.iOS-27-0" : [
              {
                "dataPath" : "/Users/me/Library/Developer/CoreSimulator/Devices/BFE8B5A4-FC91-4D33-9148-9755D340E107/data",
                "dataPathSize" : 4964089856,
                "udid" : "BFE8B5A4-FC91-4D33-9148-9755D340E107",
                "isAvailable" : true,
                "name" : "iPhone 18 Pro"
              }
            ],
            "com.apple.CoreSimulator.SimRuntime.iOS-26-0" : [
              {
                "dataPath" : "/Users/me/Library/Developer/CoreSimulator/Devices/2ED1738B-87FE-411D-9108-82FB03264E5A/data",
                "dataPathSize" : 2555953152,
                "udid" : "2ED1738B-87FE-411D-9108-82FB03264E5A",
                "isAvailable" : false,
                "availabilityError" : "runtime profile not found",
                "name" : "iPhone 17"
              }
            ],
            "iOS 12.1" : [
              {
                "udid" : "11111111-2222-3333-4444-555555555555",
                "availability" : "(unavailable, runtime profile not found)",
                "name" : "iPhone X"
              }
            ]
          }
        }"#;
        let mut gone = parse_unavailable(devices);
        gone.sort_by(|a, b| a.udid.cmp(&b.udid));
        assert_eq!(
            gone,
            [
                SimDevice {
                    udid: "11111111-2222-3333-4444-555555555555".into(),
                    name: "iPhone X".into(),
                    dir: None,
                    bytes: 0,
                },
                SimDevice {
                    udid: "2ED1738B-87FE-411D-9108-82FB03264E5A".into(),
                    name: "iPhone 17".into(),
                    dir: Some(
                        "/Users/me/Library/Developer/CoreSimulator/Devices/2ED1738B-87FE-411D-9108-82FB03264E5A"
                            .into()
                    ),
                    bytes: 2_555_953_152,
                },
            ]
        );
        assert!(parse_unavailable(b"{}").is_empty());
    }
}
