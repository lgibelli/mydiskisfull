//! Docker: what `docker system prune` would remove, item by item (Docker has
//! no dry run for it), and the images no container uses, which only `docker
//! system prune -a` removes. Read from `docker system df` (Docker's own
//! count of what it can reclaim), `docker system df -v` (each item, sized)
//! and `docker network ls`: a fraction of a second, only while Docker is
//! running, and only when you look, never in the background.
//!
//! Removed by id ([`check`], [`remove`]), not by pruning: only what you were
//! shown goes, each checked again right before, and Docker itself refuses
//! what's in use by then (a container started since, an image a container
//! uses). Volumes are never offered: they hold data. Docker Desktop keeps it
//! all in one disk image, which gives the space back to macOS a few seconds
//! later ([`settle`]).
//!
//! What Docker has had no use for in a week ([`old`]): stopped containers
//! that haven't run since, images no tag points to, networks no container
//! uses, and build cache no build has used since. The app's Clean Up removes
//! it ([`clear_old`]), starting Docker Desktop in the background for it when
//! it isn't running and quitting it again after; the daily tidy-up does too,
//! but only while Docker is running ([`answers_now`]): it never starts it.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::privacy;
use crate::space::run_within;
use crate::text::printable;

/// Docker Desktop's disk image, from the home folder.
pub const DISK: &str = "Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw";
/// Docker Desktop's folder, from the home folder: what its disk image is in.
pub const FOLDER: &str = "Library/Containers/com.docker.docker";
/// What `mdif clean` takes for Docker's items: `docker:container:<id>`.
pub const PREFIX: &str = "docker:";
/// What `mdif clean` takes for what Docker has had no use for in a week.
pub const OLD: &str = "docker:old";
/// A stopped container that hasn't run for this long is old, and so is
/// build cache no build has used for this long.
pub const OLD_AFTER: i64 = 7 * 86_400;
/// What Docker says when it isn't running, said our way.
pub const NOT_RUNNING: &str = "Docker isn't running.";

/// What Docker can remove, as `mdif docker` shows it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    /// Whether Docker answered; when it didn't, why.
    pub running: bool,
    pub why: Option<String>,
    /// What Docker Desktop's disk image takes on this disk.
    pub disk: Option<u64>,
    /// What `docker system prune` removes: stopped containers, untagged
    /// images, networks no container uses, build cache no build uses.
    pub containers: Vec<Item>,
    pub dangling: Vec<Item>,
    pub networks: Vec<Item>,
    pub cache: Option<Item>,
    /// What `-a` adds: tagged images no container uses.
    pub unused: Vec<Item>,
    /// The build cache, and how much of it the images share: that part goes
    /// only with them.
    pub cache_total: u64,
    pub cache_shared: u64,
    /// Volumes no container uses: counted, never offered.
    pub volumes: usize,
    /// What Docker counts as reclaimable: what `docker system prune -a`
    /// would free.
    pub reclaimable: u64,
    /// Docker Desktop was started for this (`mdif docker --start`): it's to
    /// be quit again after.
    pub started: bool,
}

/// One thing that can go.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    /// What to pass to `mdif clean`: `docker:container:<id>`.
    pub name: String,
    /// "postgres-dev", "node:22", "Build cache".
    pub title: String,
    /// "Exited (0) 2 days ago · postgres:16".
    pub detail: String,
    /// What deleting it frees, as Docker counts it: an image's own layers
    /// (what it shares with others goes with the last of them).
    pub bytes: u64,
    /// An image's names (`repo:tag`), which it goes by.
    #[serde(skip)]
    pub refs: Vec<String>,
}

/// How an item goes, checked.
#[derive(Debug, Clone, PartialEq)]
pub enum Removal {
    Container(String),
    /// By its names (`repo:tag`), or its id when it has none: an image with
    /// two names can't be removed by id without forcing.
    Image {
        id: String,
        refs: Vec<String>,
    },
    Network(String),
    Cache,
}

/// Where the `docker` command is: where Docker Desktop links it, Homebrew's,
/// Docker Desktop's own, or the one it installs for you alone.
pub fn command(home: &Path) -> Option<PathBuf> {
    [
        PathBuf::from("/usr/local/bin/docker"),
        PathBuf::from("/opt/homebrew/bin/docker"),
        PathBuf::from("/Applications/Docker.app/Contents/Resources/bin/docker"),
        home.join(".docker/bin/docker"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// What Docker Desktop's disk image takes on this disk (allocated: the file
/// is sparse, and far bigger than that).
pub fn disk_size(home: &Path) -> Option<u64> {
    let disk = home.join(DISK);
    if !privacy::may_open(&disk) {
        return None;
    }
    std::fs::symlink_metadata(disk)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.blocks() * 512)
}

/// Asks Docker now.
pub fn report(home: &Path) -> Report {
    let disk = disk_size(home);
    let failed = |why: String| Report {
        why: Some(why),
        disk,
        ..Report::default()
    };
    let Some(docker) = command(home) else {
        return failed("Docker isn't installed.".into());
    };
    let df = match run(
        &docker,
        &["system", "df", "-v", "--format", "{{json .}}"],
        60,
    ) {
        Ok(out) => out,
        Err(why) => return failed(why),
    };
    let totals = run(&docker, &["system", "df", "--format", "{{json .}}"], 60).unwrap_or_default();
    let networks = run(
        &docker,
        &[
            "network",
            "ls",
            "--filter",
            "dangling=true",
            "--format",
            "{{json .}}",
        ],
        30,
    )
    .unwrap_or_default();
    match parse(&df, &totals, &networks) {
        Ok(r) => Report { disk, ..r },
        Err(why) => failed(why),
    }
}

/// `docker system df -v`'s JSON, `docker system df`'s lines and `docker
/// network ls`'s lines, as a report.
pub fn parse(df: &str, totals: &str, networks: &str) -> Result<Report, String> {
    let df: Df = serde_json::from_str(df).map_err(|e| format!("Docker's answer: {e}"))?;
    let mut r = Report {
        running: true,
        ..Report::default()
    };

    for c in df.containers {
        // A stopped one: what prune removes. Running, paused or restarting
        // ones stay.
        if !matches!(c.state.as_str(), "exited" | "created" | "dead") || !id_ok(&c.id) {
            continue;
        }
        r.containers.push(Item {
            name: format!("{PREFIX}container:{}", c.id),
            title: c.names,
            detail: format!("{} · {}", c.status, c.image),
            bytes: size(&c.size),
            refs: Vec::new(),
        });
    }

    // An image with two names is listed twice: one item, both names.
    let mut images: Vec<(String, Vec<String>, String, u64)> = Vec::new();
    for i in df.images {
        // A container uses it, or Docker didn't say: it stays.
        if i.containers.trim() != "0" || !id_ok(&i.id) {
            continue;
        }
        let named = i.repository != "<none>" && i.tag != "<none>";
        let reference = named.then(|| format!("{}:{}", i.repository, i.tag));
        match images.iter_mut().find(|(id, ..)| *id == i.id) {
            Some((_, refs, ..)) => refs.extend(reference),
            None => images.push((
                i.id,
                reference.into_iter().collect(),
                i.created_since,
                size(&i.unique_size),
            )),
        }
    }
    for (id, refs, since, bytes) in images {
        let made = format!("Made {since}");
        let item = |title: String, detail: String| Item {
            name: format!("{PREFIX}image:{id}"),
            title,
            detail,
            bytes,
            refs: refs.clone(),
        };
        match refs.as_slice() {
            [] => r
                .dangling
                .push(item(format!("Untagged image {}", short(&id)), made.clone())),
            [one] => r.unused.push(item(one.clone(), made.clone())),
            [first, rest @ ..] => r.unused.push(item(
                first.clone(),
                format!("{made} · also {}", rest.join(", ")),
            )),
        }
    }

    for line in networks.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(n) = serde_json::from_str::<DfNetwork>(line) else {
            continue;
        };
        if id_ok(&n.id) {
            r.networks.push(Item {
                name: format!("{PREFIX}network:{}", n.id),
                title: n.name,
                detail: "A network no container uses".into(),
                bytes: 0,
                refs: Vec::new(),
            });
        }
    }

    r.volumes = df.volumes.iter().filter(|v| v.links.trim() == "0").count();

    let unused_cache = df.cache.iter().filter(|c| c.in_use != "true").count();
    for line in totals.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(t) = serde_json::from_str::<DfTotal>(line) else {
            continue;
        };
        let reclaimable = size(&t.reclaimable);
        if matches!(t.kind.as_str(), "Images" | "Containers" | "Build Cache") {
            r.reclaimable += reclaimable;
        }
        if t.kind == "Build Cache" {
            r.cache_total = size(&t.size);
            r.cache_shared = r.cache_total.saturating_sub(reclaimable);
            if reclaimable > 0 {
                r.cache = Some(Item {
                    name: format!("{PREFIX}build-cache"),
                    title: "Build cache".into(),
                    detail: format!(
                        "{unused_cache} entr{} no build is using: the next build is slower",
                        if unused_cache == 1 { "y" } else { "ies" }
                    ),
                    bytes: reclaimable,
                    refs: Vec::new(),
                });
            }
        }
    }

    for list in [&mut r.containers, &mut r.dangling, &mut r.unused] {
        list.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.title.cmp(&b.title)));
    }
    Ok(r)
}

/// Whether `name` is one of Docker's items (`docker:image:<id>`).
pub fn is_item(name: &Path) -> bool {
    name.to_str().is_some_and(|n| n.starts_with(PREFIX))
}

/// How `name` goes, if it's still what it was: a container still stopped,
/// an image no container uses, a network none uses, by `now`, Docker's
/// answer right before. An id may be shortened to 12 characters.
pub fn check(name: &Path, now: &Report) -> Result<Removal, String> {
    let shown = printable(&name.to_string_lossy()).into_owned();
    let rest = name
        .to_str()
        .and_then(|n| n.strip_prefix(PREFIX))
        .ok_or_else(|| format!("{shown} isn't one of Docker's."))?;
    if rest == "build-cache" {
        return now
            .cache
            .as_ref()
            .map(|_| Removal::Cache)
            .ok_or_else(|| "Docker has no build cache to delete now.".into());
    }
    let (kind, id) = rest
        .split_once(':')
        .ok_or_else(|| format!("{shown} isn't one of Docker's."))?;
    let bare = id.strip_prefix("sha256:").unwrap_or(id);
    if bare.len() < 12 || !bare.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{shown}: not an id Docker gives."));
    }
    let find = |items: &[Item], kind: &str| -> Result<Option<Item>, String> {
        let found: Vec<&Item> = items
            .iter()
            .filter(|i| {
                i.name
                    .strip_prefix(PREFIX)
                    .and_then(|n| n.strip_prefix(kind))
                    .and_then(|n| n.strip_prefix(':'))
                    .map(|n| n.strip_prefix("sha256:").unwrap_or(n))
                    .is_some_and(|n| n.starts_with(bare))
            })
            .collect();
        match found.as_slice() {
            [] => Ok(None),
            [one] => Ok(Some((*one).clone())),
            _ => Err(format!(
                "{shown}: more than one has that id; give more of it."
            )),
        }
    };
    let full = |i: &Item| i.name.splitn(3, ':').nth(2).unwrap_or_default().to_string();
    match kind {
        "container" => find(&now.containers, kind)?
            .map(|i| Removal::Container(full(&i)))
            .ok_or_else(|| format!("{shown} isn't a stopped container now.")),
        "image" => {
            let all: Vec<Item> = now.dangling.iter().chain(&now.unused).cloned().collect();
            find(&all, kind)?
                .map(|i| Removal::Image {
                    id: full(&i),
                    refs: i.refs,
                })
                .ok_or_else(|| format!("{shown} isn't an image no container uses now."))
        }
        "network" => find(&now.networks, kind)?
            .map(|i| Removal::Network(full(&i)))
            .ok_or_else(|| format!("{shown} isn't a network no container uses now.")),
        _ => Err(format!("{shown} isn't one of Docker's.")),
    }
}

/// Removes it, without forcing: Docker refuses what's in use.
pub fn remove(r: &Removal, home: &Path) -> Result<(), String> {
    let docker = command(home).ok_or("Docker isn't installed.")?;
    let (args, wait): (Vec<&str>, u64) = match r {
        Removal::Container(id) => (vec!["container", "rm", "--", id], 60),
        Removal::Image { id, refs } if refs.is_empty() => (vec!["image", "rm", "--", id], 120),
        Removal::Image { refs, .. } => {
            let mut a = vec!["image", "rm", "--"];
            a.extend(refs.iter().map(String::as_str));
            (a, 120)
        }
        Removal::Network(id) => (vec!["network", "rm", "--", id], 60),
        Removal::Cache => (vec!["builder", "prune", "--all", "--force"], 600),
    };
    run(&docker, &args, wait).map(|_| ())
}

/// Waits for Docker Desktop's disk image to give back what was deleted in
/// it, which it does a few seconds later, and says how much it did.
/// `before` is what it took before. One that hasn't shrunk in 30 seconds
/// isn't waited for.
pub fn settle(home: &Path, before: u64) -> u64 {
    let start = Instant::now();
    let mut last = before;
    let mut shrank_at = None;
    while start.elapsed() < Duration::from_secs(90) {
        std::thread::sleep(Duration::from_secs(1));
        let now = disk_size(home).unwrap_or(last);
        if now < last {
            last = now;
            shrank_at = Some(Instant::now());
        }
        match shrank_at {
            // Done once it stays the same for a few seconds.
            Some(t) if t.elapsed() >= Duration::from_secs(4) => break,
            None if start.elapsed() >= Duration::from_secs(30) => break,
            _ => {}
        }
    }
    before.saturating_sub(last)
}

/// Runs `docker args`, within `wait` seconds: its output, or why not.
fn run(docker: &Path, args: &[&str], wait: u64) -> Result<String, String> {
    let mut cmd = Command::new(docker);
    cmd.args(args);
    let out = run_within(cmd, Duration::from_secs(wait))
        .ok_or("Docker didn't answer in time. Is Docker Desktop starting or stuck?")?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    let said = said
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if not_running(said) {
        return Err(NOT_RUNNING.into());
    }
    Err(format!("Docker: {}", printable(said)))
}

/// Whether Docker said it isn't running: in Docker 28's words and before,
/// or in 29's ("failed to connect to the docker API at unix://…: connect:
/// no such file or directory").
pub fn not_running(said: &str) -> bool {
    said.contains("Cannot connect to the Docker daemon")
        || said.contains("docker daemon is not running")
        || said.contains("Is the docker daemon running")
        || said.contains("failed to connect to the docker API")
}

/// Whether Docker would answer now, by connecting to its socket: a few
/// microseconds, so the watcher may ask every few minutes. Docker Desktop's
/// socket, then the usual one.
pub fn answers_now(home: &Path) -> bool {
    [
        home.join(".docker/run/docker.sock"),
        PathBuf::from("/var/run/docker.sock"),
    ]
    .iter()
    .any(|s| std::os::unix::net::UnixStream::connect(s).is_ok())
}

/// Starts Docker Desktop in the background when Docker isn't running, and
/// waits for it to answer, three minutes at most: whether it was started
/// here (and so is to be quit after, [`stop`]).
pub fn start(home: &Path) -> Result<bool, String> {
    let docker = command(home).ok_or("Docker isn't installed.")?;
    if answers(&docker) {
        return Ok(false);
    }
    // Docker Desktop's own command waits until it's up. Without it (before
    // Docker Desktop 4.37), the app, opened hidden.
    if run(&docker, &["desktop", "start", "--timeout", "180"], 200).is_err() {
        let app = Path::new("/Applications/Docker.app");
        if !app.is_dir() {
            return Err(format!(
                "{NOT_RUNNING} Start it, and MyDiskIsFull cleans it then."
            ));
        }
        let opened = Command::new("/usr/bin/open")
            .args(["-g", "-j", "-a"])
            .arg(app)
            .status()
            .is_ok_and(|s| s.success());
        if !opened {
            return Err(format!("{NOT_RUNNING} Docker Desktop didn't start."));
        }
    }
    let begun = Instant::now();
    while begun.elapsed() < Duration::from_secs(180) {
        if answers(&docker) {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    Err("Docker Desktop didn't start in three minutes.".into())
}

/// Quits Docker Desktop, once it was started for a clean-up.
pub fn stop(home: &Path) -> Result<(), String> {
    let docker = command(home).ok_or("Docker isn't installed.")?;
    run(&docker, &["desktop", "stop", "--timeout", "60"], 90).map(|_| ())
}

fn answers(docker: &Path) -> bool {
    run(docker, &["info", "--format", "{{.ServerVersion}}"], 30).is_ok()
}

/// What Docker has had no use for in [`OLD_AFTER`], from `r` (Docker's
/// answer) and when each stopped container last ran (`ran`, by id: its
/// end, or when it was made if it never ran): stopped containers that
/// haven't run since, images no tag points to and networks no container
/// uses. Not tagged images, nor volumes, nor a container Docker didn't say
/// when it ran. Build cache goes by Docker's own filter
/// ([`prune_old_cache`]).
pub fn old(r: &Report, ran: &[(String, i64)], now: i64) -> Vec<Item> {
    let ended = |item: &Item| {
        let id = item.name.strip_prefix(&format!("{PREFIX}container:"))?;
        ran.iter()
            .find(|(full, _)| full.starts_with(id) || id.starts_with(full.as_str()))
            .map(|(_, at)| *at)
    };
    r.containers
        .iter()
        .filter(|c| ended(c).is_some_and(|at| now - at >= OLD_AFTER))
        .chain(&r.dangling)
        .chain(&r.networks)
        .cloned()
        .collect()
}

/// When each container last ran: `docker container inspect`'s answer, a
/// line each, `<id> <FinishedAt> <Created>`.
pub fn parse_ran(out: &str) -> Vec<(String, i64)> {
    out.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let (id, finished, created) = (w.next()?, w.next()?, w.next()?);
            // Never ran: Docker's zero time.
            let at = time(finished)
                .filter(|&t| t > 0)
                .or_else(|| time(created))?;
            id_ok(id).then(|| (id.to_string(), at))
        })
        .collect()
}

/// Docker's times, to the second: "2026-11-02T10:11:12.123456789Z".
fn time(s: &str) -> Option<i64> {
    let (whole, rest) = s.split_at_checked(19)?;
    let zone = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    crate::space::parse_time(&format!("{whole}{zone}"))
}

/// What's old now ([`old`]): Docker asked, then when its stopped
/// containers last ran.
pub fn old_now(home: &Path, now: i64) -> Result<Vec<Item>, String> {
    let docker = command(home).ok_or("Docker isn't installed.")?;
    let r = report(home);
    if let Some(why) = r.why {
        return Err(why);
    }
    let ids: Vec<&str> = r
        .containers
        .iter()
        .filter_map(|c| c.name.strip_prefix(&format!("{PREFIX}container:")))
        .collect();
    let ran = if ids.is_empty() {
        Vec::new()
    } else {
        let mut args = vec![
            "container",
            "inspect",
            "--format",
            "{{.Id}} {{.State.FinishedAt}} {{.Created}}",
            "--",
        ];
        args.extend(&ids);
        parse_ran(&run(&docker, &args, 60)?)
    };
    Ok(old(&r, &ran, now))
}

/// Removes one of what [`old`] found, without forcing.
pub fn remove_item(item: &Item, home: &Path) -> Result<(), String> {
    let rest = item.name.strip_prefix(PREFIX).unwrap_or_default();
    let removal = match rest.split_once(':') {
        Some(("container", id)) => Removal::Container(id.into()),
        Some(("image", id)) => Removal::Image {
            id: id.into(),
            refs: item.refs.clone(),
        },
        Some(("network", id)) => Removal::Network(id.into()),
        _ => return Err(format!("{}: not one of Docker's.", printable(&item.name))),
    };
    remove(&removal, home)
}

/// Deletes the build cache no build has used for [`OLD_AFTER`]: what Docker
/// says it reclaimed.
pub fn prune_old_cache(home: &Path) -> Result<u64, String> {
    let docker = command(home).ok_or("Docker isn't installed.")?;
    let until = format!("until={}h", OLD_AFTER / 3600);
    let out = run(
        &docker,
        &["builder", "prune", "--force", "--filter", &until],
        600,
    )?;
    // "Total:\t1.2GB", or "Total reclaimed space: 1.2GB" before.
    Ok(out
        .lines()
        .rev()
        .find_map(|l| {
            let l = l.trim();
            l.strip_prefix("Total reclaimed space:")
                .or_else(|| l.strip_prefix("Total:"))
        })
        .map(|n| size(n.trim()))
        .unwrap_or(0))
}

/// What [`clear_old`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cleared {
    pub containers: usize,
    pub images: usize,
    pub networks: usize,
    /// What the build cache gave back, as Docker counts it.
    pub cache: u64,
    /// Docker Desktop was started for it.
    pub started: bool,
    /// Started for it, and still running: it didn't quit when asked.
    pub left_running: bool,
    /// What Docker refused (in use by then), or couldn't do.
    pub failed: Vec<String>,
}

impl Cleared {
    /// "Removed 3 stopped containers and 2 untagged images, and 1.2 GB of
    /// build cache. Docker Desktop was started for it, and quit again."
    pub fn said(&self) -> String {
        let n = |k: usize, one: &str, many: &str| match k {
            0 => None,
            1 => Some(format!("1 {one}")),
            k => Some(format!("{k} {many}")),
        };
        let mut what: Vec<String> = [
            n(self.containers, "stopped container", "stopped containers"),
            n(self.images, "untagged image", "untagged images"),
            n(self.networks, "unused network", "unused networks"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if self.cache > 0 {
            what.push(format!(
                "{} of build cache",
                crate::units::bytes(self.cache)
            ));
        }
        let mut out = match what.as_slice() {
            [] => "Nothing in Docker had gone unused for a week.".to_string(),
            [one] => format!("Removed {one} from Docker."),
            [rest @ .., last] => format!("Removed {} and {last} from Docker.", rest.join(", ")),
        };
        if self.started {
            out.push_str(if self.left_running {
                " Docker Desktop was started for it, and is still running: quit it from its menu."
            } else {
                " Docker Desktop was started for it, and quit again."
            });
        }
        out
    }
}

/// Removes what Docker has had no use for in a week ([`old`], and old build
/// cache), starting Docker Desktop for it if it isn't running and quitting
/// it after; waits for its disk image to give the space back first
/// ([`settle`]). Refused only when nothing could be asked; what Docker
/// refuses (in use by then) is in `failed`.
pub fn clear_old(home: &Path, now: i64) -> Result<Cleared, String> {
    let started = start(home)?;
    let mut c = Cleared {
        started,
        ..Cleared::default()
    };
    let before = disk_size(home);
    let done = old_now(home, now).map(|items| {
        for i in &items {
            match remove_item(i, home) {
                Ok(()) if i.name.contains(":container:") => c.containers += 1,
                Ok(()) if i.name.contains(":image:") => c.images += 1,
                Ok(()) => c.networks += 1,
                Err(why) => c.failed.push(format!("{}: {why}", printable(&i.title))),
            }
        }
        match prune_old_cache(home) {
            Ok(n) => c.cache = n,
            Err(why) => c.failed.push(format!("Build cache: {why}")),
        }
    });
    let removed = c.containers + c.images + c.networks > 0 || c.cache > 0;
    // The disk image shrinks only while Docker runs.
    if let (Some(before), true) = (before, removed) {
        settle(home, before);
    }
    if started && stop(home).is_err() {
        c.left_running = true;
    }
    done.map(|()| c)
}

/// An id as Docker shows it: 12 characters, without `sha256:`.
fn short(id: &str) -> &str {
    let bare = id.strip_prefix("sha256:").unwrap_or(id);
    bare.get(..12).unwrap_or(bare)
}

/// An id that is only what Docker gives: hex, after `sha256:` for an image.
fn id_ok(id: &str) -> bool {
    let bare = id.strip_prefix("sha256:").unwrap_or(id);
    bare.len() >= 12 && bare.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Docker's sizes, in its units (powers of 1000): "2.051GB", "0B",
/// "12.3kB (virtual 139MB)"; anything else 0.
pub fn size(s: &str) -> u64 {
    let s = s
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches('*');
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (n, unit) = s.split_at(split);
    let Ok(n) = n.parse::<f64>() else {
        return 0;
    };
    let scale = match unit {
        "B" | "" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        "PB" => 1e15,
        _ => return 0,
    };
    (n * scale).round() as u64
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Df {
    #[serde(rename = "Images")]
    images: Vec<DfImage>,
    #[serde(rename = "Containers")]
    containers: Vec<DfContainer>,
    #[serde(rename = "Volumes")]
    volumes: Vec<DfVolume>,
    #[serde(rename = "BuildCache")]
    cache: Vec<DfCache>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
struct DfImage {
    #[serde(rename = "ID")]
    id: String,
    repository: String,
    tag: String,
    containers: String,
    created_since: String,
    unique_size: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
struct DfContainer {
    #[serde(rename = "ID")]
    id: String,
    names: String,
    image: String,
    state: String,
    status: String,
    size: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
struct DfVolume {
    links: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
struct DfCache {
    in_use: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
struct DfTotal {
    #[serde(rename = "Type")]
    kind: String,
    size: String,
    reclaimable: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
struct DfNetwork {
    #[serde(rename = "ID")]
    id: String,
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX: &str = "7185107c3261e393732a218940febb8dac8d1c4655c11082dba754d4c2556cb4";

    fn df() -> String {
        format!(
            r#"{{"Images":[
              {{"Containers":"0","CreatedSince":"2 days ago","ID":"sha256:7ed511951558d11de6d1f96e48bc7bd79ad5ab8d501d398e7844cb36c834ebd0","Repository":"shop-builder","Tag":"m1","Size":"2.28GB","SharedSize":"230.4MB","UniqueSize":"2.051GB"}},
              {{"Containers":"0","CreatedSince":"2 days ago","ID":"sha256:7ed511951558d11de6d1f96e48bc7bd79ad5ab8d501d398e7844cb36c834ebd0","Repository":"registry.example/shop-builder","Tag":"0a88d893","Size":"2.28GB","UniqueSize":"2.051GB"}},
              {{"Containers":"0","CreatedSince":"3 weeks ago","ID":"sha256:4105e2bb7b57aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","Repository":"<none>","Tag":"<none>","Size":"139MB","UniqueSize":"139MB"}},
              {{"Containers":"1","CreatedSince":"3 weeks ago","ID":"sha256:c2747b924301bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","Repository":"postgres","Tag":"16","Size":"400MB","UniqueSize":"400MB"}},
              {{"Containers":"N/A","ID":"sha256:aa4f71e6fa4acccccccccccccccccccccccccccccccccccccccccccccccccccc","Repository":"odd","Tag":"x","UniqueSize":"1GB"}}
            ],
            "Containers":[
              {{"ID":"{HEX}","Names":"db-old","Image":"postgres:16","State":"exited","Status":"Exited (0) 2 days ago","Size":"12.3kB (virtual 400MB)"}},
              {{"ID":"8185107c3261e393732a218940febb8dac8d1c4655c11082dba754d4c2556cb4","Names":"web","Image":"nginx","State":"running","Status":"Up 3 hours","Size":"1MB"}}
            ],
            "Volumes":[{{"Links":"0","Name":"a"}},{{"Links":"1","Name":"b"}}],
            "BuildCache":[{{"InUse":"false","Size":"0B"}},{{"InUse":"true","Size":"0B"}}]}}"#
        )
    }

    const TOTALS: &str = r#"{"Reclaimable":"2.19GB (80%)","Size":"11.25GB","Type":"Images"}
{"Reclaimable":"12.3kB","Size":"1MB","Type":"Containers"}
{"Reclaimable":"0B","Size":"0B","Type":"Local Volumes"}
{"Reclaimable":"300MB","Size":"10.49GB","Type":"Build Cache"}
"#;

    const NETWORKS: &str = r#"{"ID":"63b2abd332b3","Name":"old-net","Driver":"bridge"}
"#;

    #[test]
    fn docker_says_what_prune_would_remove_and_what_only_dash_a_would() {
        let r = parse(&df(), TOTALS, NETWORKS).unwrap();
        assert!(r.running);
        // The stopped container only, sized by what it wrote.
        let names: Vec<&str> = r.containers.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(names, ["db-old"]);
        assert_eq!(r.containers[0].bytes, 12_300);
        assert_eq!(
            r.containers[0].detail,
            "Exited (0) 2 days ago · postgres:16"
        );
        // Untagged, then named: one item for an image with two names; not
        // one a container uses, nor one Docker didn't say about.
        assert_eq!(r.dangling.len(), 1);
        assert_eq!(r.dangling[0].title, "Untagged image 4105e2bb7b57");
        assert_eq!(r.unused.len(), 1);
        assert_eq!(r.unused[0].title, "shop-builder:m1");
        assert_eq!(
            r.unused[0].detail,
            "Made 2 days ago · also registry.example/shop-builder:0a88d893"
        );
        assert_eq!(r.unused[0].bytes, 2_051_000_000);
        assert_eq!(r.networks[0].name, "docker:network:63b2abd332b3");
        // The cache: what only it holds; the rest goes with the images.
        assert_eq!(r.cache.as_ref().unwrap().bytes, 300_000_000);
        assert_eq!(r.cache_shared, 10_190_000_000);
        assert_eq!(r.volumes, 1);
        assert_eq!(r.reclaimable, 2_190_000_000 + 12_300 + 300_000_000);
    }

    #[test]
    fn an_item_goes_only_while_it_is_still_what_was_shown() {
        let r = parse(&df(), TOTALS, NETWORKS).unwrap();
        let c = |n: &str| check(Path::new(n), &r);
        assert_eq!(
            c(&format!("docker:container:{HEX}")),
            Ok(Removal::Container(HEX.into()))
        );
        // Shortened, as Docker shows it.
        assert_eq!(
            c("docker:container:7185107c3261"),
            Ok(Removal::Container(HEX.into()))
        );
        // Running now: refused.
        assert!(c("docker:container:8185107c3261").is_err());
        // An image with two names goes by both; an untagged one by id.
        assert_eq!(
            c("docker:image:sha256:7ed511951558"),
            Ok(Removal::Image {
                id: "sha256:7ed511951558d11de6d1f96e48bc7bd79ad5ab8d501d398e7844cb36c834ebd0"
                    .into(),
                refs: vec![
                    "shop-builder:m1".into(),
                    "registry.example/shop-builder:0a88d893".into()
                ],
            })
        );
        assert!(matches!(
            c("docker:image:4105e2bb7b57"),
            Ok(Removal::Image { refs, .. }) if refs.is_empty()
        ));
        // One a container uses.
        assert!(c("docker:image:c2747b924301").is_err());
        assert_eq!(
            c("docker:network:63b2abd332b3"),
            Ok(Removal::Network("63b2abd332b3".into()))
        );
        assert_eq!(c("docker:build-cache"), Ok(Removal::Cache));
        // Never anything Docker would read as an option, nor a short id.
        assert!(c("docker:container:--force").is_err());
        assert!(c("docker:container:7185").is_err());
        assert!(c("docker:volume:aaaaaaaaaaaa").is_err());
    }

    #[test]
    fn the_apps_docker_fixture_has_the_fields_docker_prints() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../app/Tests/docker.json")).unwrap();
        let real = serde_json::to_value(parse(&df(), TOTALS, NETWORKS).unwrap()).unwrap();
        let keys = |v: &serde_json::Value| {
            v.as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(keys(&fixture), keys(&real));
        let item = keys(&real["containers"][0]);
        for list in ["containers", "dangling", "networks", "unused"] {
            for i in fixture[list].as_array().unwrap() {
                assert_eq!(keys(i), item, "{list}");
            }
        }
        assert_eq!(keys(&fixture["cache"]), item);
    }

    #[test]
    fn docker_sizes_are_in_powers_of_a_thousand() {
        assert_eq!(size("2.051GB"), 2_051_000_000);
        assert_eq!(size("0B"), 0);
        assert_eq!(size("12.3kB (virtual 139MB)"), 12_300);
        assert_eq!(size("994.3MB*"), 994_300_000);
        assert_eq!(size("9.075GB (80%)"), 9_075_000_000);
        assert_eq!(size("N/A"), 0);
    }

    #[test]
    fn docker_not_running_is_said_in_every_version() {
        for said in [
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?",
            "failed to connect to the docker API at unix:///Users/me/.docker/run/docker.sock; check if the path is correct and if the daemon is running: dial unix /Users/me/.docker/run/docker.sock: connect: no such file or directory",
        ] {
            assert!(not_running(said), "{said}");
        }
        assert!(!not_running(
            "Error response from daemon: conflict: unable to remove"
        ));
    }

    #[test]
    fn what_docker_hasnt_used_in_a_week_is_old() {
        let now = crate::space::parse_time("2026-11-09T12:00:00Z").unwrap();
        // Ended 10 days ago; never ran, made 2 days ago; one Docker
        // doesn't know.
        let out = format!(
            "{HEX} 2026-10-30T09:15:02.123456789Z 2026-10-01T08:00:00.5Z\n\
             9185107c3261e393732a218940febb8dac8d1c4655c11082dba754d4c2556cb4 0001-01-01T00:00:00Z 2026-11-07T08:00:00Z\n\
             not-an-id 2026-10-30T09:15:02Z 2026-10-01T08:00:00Z\n"
        );
        let ran = parse_ran(&out);
        assert_eq!(ran.len(), 2);
        assert_eq!(
            ran[0].1,
            crate::space::parse_time("2026-10-30T09:15:02Z").unwrap()
        );
        assert_eq!(
            ran[1].1,
            crate::space::parse_time("2026-11-07T08:00:00Z").unwrap()
        );

        let r = parse(&df(), TOTALS, NETWORKS).unwrap();
        let names: Vec<String> = old(&r, &ran, now).into_iter().map(|i| i.title).collect();
        // The stopped container ended 10 days ago, the untagged image, the
        // network; never the tagged image no container uses.
        assert_eq!(names, ["db-old", "Untagged image 4105e2bb7b57", "old-net"]);
        // Stopped yesterday: it stays.
        let lately = [(HEX.to_string(), now - 86_400)];
        let names: Vec<String> = old(&r, &lately, now).into_iter().map(|i| i.title).collect();
        assert_eq!(names, ["Untagged image 4105e2bb7b57", "old-net"]);
        // Docker didn't say when it ran: it stays too.
        assert_eq!(old(&r, &[], now).len(), 2);
    }

    #[test]
    fn what_was_cleared_is_said_in_a_sentence() {
        let c = Cleared {
            containers: 3,
            images: 1,
            cache: 1_200_000_000,
            started: true,
            ..Cleared::default()
        };
        assert_eq!(
            c.said(),
            "Removed 3 stopped containers, 1 untagged image and 1.2 GB of build cache from \
             Docker. Docker Desktop was started for it, and quit again."
        );
        assert_eq!(
            Cleared::default().said(),
            "Nothing in Docker had gone unused for a week."
        );
        let left = Cleared {
            networks: 1,
            started: true,
            left_running: true,
            ..Cleared::default()
        };
        assert!(
            left.said()
                .starts_with("Removed 1 unused network from Docker.")
        );
        assert!(left.said().ends_with("quit it from its menu."));
    }
}
