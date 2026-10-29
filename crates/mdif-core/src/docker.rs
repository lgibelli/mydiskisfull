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
    if said.contains("Cannot connect to the Docker daemon")
        || said.contains("docker daemon is not running")
        || said.contains("Is the docker daemon running")
    {
        return Err("Docker isn't running. Open Docker Desktop, then look again.".into());
    }
    Err(format!("Docker: {}", printable(said)))
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
}
