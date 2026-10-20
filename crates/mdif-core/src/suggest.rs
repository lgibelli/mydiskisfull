//! Suggestions: for each thing that fills the disk, what keeps it from
//! coming back.
//!
//! Every cause the watcher can point at maps to a fix: a runaway folder or
//! process, a folder that grew, a cleanup that refilled, or something on the
//! disk now (an AI model, old snapshots, swap). The fix is a tweak when a
//! setting can stop it, otherwise the command to run or the steps to take
//! in the app, or the advice to leave it alone when it rebuilds itself.
//! Deleting something once is never the whole answer.
//!
//! Commands are printed for the user to run, never run here. Paths in them
//! are shell-quoted, since a folder name can contain anything.

use std::path::{Component, Path};

use anyhow::Result;
use serde::Serialize;

use crate::forecast::{self, DAY, HOUR, Params};
use crate::incidents::{Incident, Kind};
use crate::owners::{self, ARTIFACTS};
use crate::store::Store;
use crate::tweaks::{self, After, CATALOGUE, CHROME_MODEL, Status};
use crate::units::bytes;
use crate::{apfs, scan};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "lowercase")]
pub enum Fix {
    /// A tweak from the catalogue.
    Tweak(&'static str),
    /// A command to run once, and what to know first.
    Run { command: String, note: String },
    /// What to change in an app or in System Settings.
    Steps(String),
    /// Nothing worth doing by hand: it refills by itself.
    Leave(String),
}

impl Fix {
    /// The fix as one line, to print under its cause.
    pub fn line(&self) -> String {
        match self {
            Fix::Tweak(id) => {
                let title = tweaks::find(id).map_or(*id, |t| t.title);
                format!("{title} (`mdif tweaks apply {id}`).")
            }
            Fix::Run { command, note } => format!("`{command}`. {note}"),
            Fix::Steps(s) | Fix::Leave(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    /// The cause, in one sentence with its evidence.
    pub cause: String,
    pub fix: Fix,
    /// Space it concerns now, when measured.
    pub space: Option<u64>,
    /// How fast it grows, bytes a day, when known.
    pub per_day: Option<f64>,
    /// Incidents come first, then what was measured, biggest first.
    #[serde(skip)]
    rank: (u8, u64),
    #[serde(skip)]
    key: String,
}

const GB: u64 = 1_000_000_000;

/// The fix for growth in `owner`, a folder from [`owners::Rules`].
pub fn for_folder(owner: &Path, home: &Path) -> Fix {
    let name = owner
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Ok(rel) = owner.strip_prefix(home) {
        let r = rel.to_string_lossy();
        let lower = r.to_lowercase();
        match r.as_ref() {
            "Library/Application Support/Google" if home.join(CHROME_MODEL).exists() => {
                return Fix::Tweak("chrome-ai-model");
            }
            "Library/Caches/Google" | "Library/Caches/com.google.Chrome" => {
                return Fix::Tweak("chrome-cache");
            }
            ".gradle" => return Fix::Tweak("gradle-caches"),
            ".claude" => return Fix::Tweak("claude-transcripts"),
            "Library/Caches/Homebrew" => return Fix::Tweak("homebrew-downloads"),
            ".Trash" => return Fix::Tweak("trash-auto-empty"),
            "Library/Messages" => return Fix::Steps(MESSAGES.into()),
            _ => {}
        }
        // Owners here are two levels down (`CoreSimulator/Devices`, `Xcode/DerivedData`).
        if r.starts_with("Library/Developer/CoreSimulator") {
            return Fix::Run {
                command: "xcrun simctl delete unavailable".into(),
                note: "Removes simulators of runtimes no longer installed; Xcode → Settings → \
                       Components lists the runtimes themselves."
                    .into(),
            };
        }
        if r.starts_with("Library/Developer/Xcode") {
            return Fix::Steps(XCODE.into());
        }
        if r.starts_with("Library/") && lower.contains("whatsapp") {
            return Fix::Steps(WHATSAPP.into());
        }
        if r.starts_with("Library/Containers/com.docker.docker")
            || r.starts_with("Library/Group Containers/group.com.docker")
        {
            return docker();
        }
        if r.starts_with("Pictures/") && name.ends_with(".photoslibrary") {
            return Fix::Steps(PHOTOS.into());
        }
        if r.starts_with("Library/Mobile Documents") {
            return Fix::Steps(ICLOUD.into());
        }
        if r.starts_with("Library/CloudStorage/") {
            return Fix::Steps(format!(
                "{name} keeps a local copy of what you open or pin. In Finder, Control-click \
                 the folders you don't need offline and choose Free Up Space (they stay in the \
                 cloud, and download again when opened)."
            ));
        }
        if r.starts_with("Library/Logs/") {
            return Fix::Steps(format!(
                "{name} is writing a lot of logs. A debug log level left on, or something \
                 failing over and over, is the usual cause; look in its settings, or restart it."
            ));
        }
        if r.starts_with("Library/Caches/") {
            return Fix::Leave(CACHE.into());
        }
    }
    let parts: Vec<&str> = owner
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    if parts.windows(2).any(|w| w == [".claude", "worktrees"]) {
        return Fix::Steps(WORKTREE.into());
    }
    if ARTIFACTS.contains(&name.as_str())
        && let Some(project) = owner.parent()
    {
        return artifact(&name, project);
    }
    // vc/<id>/{C,T,X}/<app>: macOS's per-user caches and temporary files.
    if let Some(i) = parts
        .windows(3)
        .position(|w| w == ["private", "var", "folders"])
    {
        match parts.get(i + 5) {
            Some(&"C") => return Fix::Leave(CACHE.into()),
            Some(&"T") => {
                return Fix::Steps(format!(
                    "Temporary files of {name}. Quitting it usually deletes them, and a \
                     restart clears what's left."
                ));
            }
            _ => {}
        }
    }
    if owner == Path::new("/private/var/vm") {
        return Fix::Steps(SWAP.into());
    }
    Fix::Steps(
        "`mdif writers` shows which processes write the most; the app behind it may have a \
         setting for how much it keeps."
            .into(),
    )
}

/// The fix for a process that keeps rewriting data it doesn't keep.
pub fn for_process(name: &str, home: &Path) -> Fix {
    if name == "cfprefsd" {
        return prefs_churn(home, std::time::SystemTime::now());
    }
    // Only names we know go into a command: the name comes from the process
    // table, and anything could be in it.
    let app = match name {
        "OneDrive" | "OneDrive File Provider" | "OneDriveStandaloneUpdater" => Some("OneDrive"),
        "Dropbox" => Some("Dropbox"),
        "Google Drive" => Some("Google Drive"),
        _ => None,
    };
    if let Some(app) = app {
        return Fix::Run {
            command: format!("osascript -e 'quit app \"{app}\"'; sleep 5; open -a '{app}'"),
            note: format!(
                "A sync that is stuck usually starts over cleanly. If the writes come back, \
                 sign out of {app} and in again."
            ),
        };
    }
    if matches!(name, "bird" | "cloudd" | "fileproviderd") {
        return Fix::Steps(
            "iCloud's sync is stuck on something. A restart usually clears it; if not, sign \
             out of iCloud and in again."
                .into(),
        );
    }
    Fix::Steps(format!(
        "Quit and reopen {}. If it starts again, something in it is stuck (a sync, a log \
         level, a crash loop): look at its settings or logs.",
        crate::text::printable(name)
    ))
}

/// cfprefsd saves preferences for other apps, so its churn is an app saving
/// its settings over and over, each save rewriting the whole file. The
/// biggest preferences file rewritten in the last two minutes is the likely
/// one (on the Mac this was found on, a 2.8 MB file every few seconds).
fn prefs_churn(home: &Path, now: std::time::SystemTime) -> Fix {
    let mut dirs = vec![home.join("Library/Preferences")];
    for (root, sub) in [
        ("Library/Containers", "Data/Library/Preferences"),
        ("Library/Group Containers", "Library/Preferences"),
    ] {
        if let Ok(rd) = std::fs::read_dir(home.join(root)) {
            dirs.extend(rd.flatten().map(|e| e.path().join(sub)));
        }
    }
    let recent = std::time::Duration::from_secs(120);
    let busiest = dirs
        .iter()
        .filter(|d| crate::privacy::may_open(d))
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|rd| rd.flatten())
        .filter(|e| e.path().extension().is_some_and(|x| x == "plist"))
        .filter_map(|e| {
            // Some containers link to the shared file; count it there.
            let m = std::fs::symlink_metadata(e.path()).ok()?;
            let age = now.duration_since(m.modified().ok()?).unwrap_or_default();
            (m.is_file() && age < recent).then(|| (m.len(), e.path()))
        })
        .max();
    match busiest {
        Some((size, path)) if size >= 100_000 => {
            let domain = path
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Fix::Steps(format!(
                "cfprefsd saves settings for other apps. The likely one here is {domain}: its \
                 settings file ({}) was just rewritten, and every save rewrites all of it. \
                 Quit that app when you don't need it, and tell its developer.",
                bytes(size)
            ))
        }
        _ => Fix::Steps(
            "cfprefsd saves settings for other apps, so one of them is saving its settings \
             over and over. The file that changes most often names it: \
             `ls -lt ~/Library/Preferences ~/Library/Containers/*/Data/Library/Preferences | \
             head`."
                .into(),
        ),
    }
}

pub fn for_incident(i: &Incident, home: &Path) -> Fix {
    settled(
        match i.kind {
            Kind::Growth => for_folder(Path::new(&i.subject), home),
            Kind::Churn => for_process(&i.subject, home),
        },
        home,
    )
}

/// `fix`, unless it is a tweak that is already applied or waiting for
/// approval: then what to do about that instead.
pub fn settled(fix: Fix, home: &Path) -> Fix {
    let Fix::Tweak(id) = fix else {
        return fix;
    };
    let Some(t) = tweaks::find(id) else {
        return fix;
    };
    match t.status(home) {
        Ok(Status::Applied) => Fix::Steps(format!(
            "\"{}\" is already applied; `mdif tweaks` shows whether it is working.",
            t.title
        )),
        Ok(Status::Pending) => Fix::Steps(format!(
            "\"{}\" is waiting for you to install its profile in System Settings → {}.",
            t.title,
            crate::profile::settings_place()
        )),
        _ => fix,
    }
}

/// Everything worth doing now, most useful first: from the watcher's
/// history when there is one, and from a look at the disk.
pub fn gather(
    home: &Path,
    volume: &Path,
    store: Option<&Store>,
    now: i64,
) -> Result<Vec<Suggestion>> {
    mdif_macos::fs::forbid_dataless_materialization()?;
    let st = mdif_macos::fs::statfs(volume)?;
    let mut tight = st.avail * 100 < st.total * 15;
    let mut out = Vec::new();

    if let Some(store) = store {
        let pts = store.space_since(&st.mount_point, now - 400 * DAY)?;
        let f = forecast::forecast(&pts, &Params::default());
        tight |= f
            .as_ref()
            .and_then(|f| f.full_in.as_ref())
            .is_some_and(|fi| fi.days < 30.0);
        from_history(store, home, now, &mut out)?;
    }
    from_tweaks(store, home, now, tight, &mut out);
    if tight {
        from_system(home, &st.mount_point, &mut out);
    }

    out.sort_by(|a, b| a.rank.cmp(&b.rank).then(b.space.cmp(&a.space)));
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert(s.key.clone()));
    Ok(out)
}

fn from_history(store: &Store, home: &Path, now: i64, out: &mut Vec<Suggestion>) -> Result<()> {
    let show = |p: &str| display(Path::new(p), home);
    for e in store.incidents_since(now)? {
        if e.closed.is_some() {
            continue;
        }
        let i = &e.incident;
        let fix = for_incident(i, home);
        out.push(Suggestion {
            cause: i.sentence(Some(home)),
            key: key_for(&fix, &i.subject),
            fix,
            space: i.size,
            per_day: Some(i.per_day),
            rank: (0, 0),
        });
    }

    // What grew, over the time the watcher has been watching (up to a week).
    let first = store.sample_times(now - 7 * DAY)?.first().copied();
    if let Some(first) = first.filter(|&t| now - t >= 6 * HOUR) {
        let span = now - first;
        for (owner, grown) in store.growth_since(first)?.into_iter().take(8) {
            if grown < GB as i64 {
                break;
            }
            let fix = settled(for_folder(Path::new(&owner), home), home);
            let per_day = grown as f64 * DAY as f64 / span as f64;
            out.push(Suggestion {
                cause: format!(
                    "{} grew {} in the last {} ({} a day).",
                    show(&owner),
                    bytes(grown as u64),
                    ago(span),
                    bytes(per_day as u64)
                ),
                key: key_for(&fix, &owner),
                fix,
                space: None,
                per_day: Some(per_day),
                rank: (1, u64::MAX - (per_day * 30.0) as u64),
            });
        }
    }

    // Cleanups that came back.
    for r in owners::regrowth(&store.growth_series(now - 30 * DAY)?, GB) {
        if r.share() < 0.5 {
            continue;
        }
        let fix = match settled(for_folder(Path::new(&r.owner), home), home) {
            Fix::Steps(s) if s.starts_with("`mdif writers`") => Fix::Leave(
                "It refills by itself: leave it, or cap it in the app that writes it.".into(),
            ),
            f => f,
        };
        out.push(Suggestion {
            cause: format!(
                "{} was cleared of {} {} ago and {} came back since: clearing it by hand \
                 doesn't last.",
                show(&r.owner),
                bytes(r.freed),
                ago(now - r.at),
                bytes(r.back.max(0) as u64)
            ),
            key: key_for(&fix, &r.owner),
            fix,
            space: Some(r.back.max(0) as u64),
            per_day: None,
            rank: (1, u64::MAX - r.back.max(0) as u64),
        });
    }
    Ok(())
}

/// Tweaks that would help, judged by what their folders hold now (one with
/// nothing to measure, like Apple Intelligence, only when space is tight),
/// and applied ones that still need something or were undone.
fn from_tweaks(
    store: Option<&Store>,
    home: &Path,
    now: i64,
    tight: bool,
    out: &mut Vec<Suggestion>,
) {
    for t in CATALOGUE {
        // A setting that can't be read is no reason to fail the rest.
        let Ok(status) = t.status(home) else {
            continue;
        };
        let check = store.and_then(|s| t.check(home, s, now).ok().flatten());
        let size = check
            .as_ref()
            .and_then(|c| c.size_now())
            .or_else(|| t.size(home));
        let mut push = |cause: String, fix: Fix, rank: (u8, u64)| {
            out.push(Suggestion {
                cause,
                fix,
                space: size,
                per_day: None,
                rank,
                key: format!("tweak:{}", t.id),
            })
        };
        match status {
            Status::Absent(_) => {}
            Status::NotApplied | Status::Partial => {
                if let Some(c) = &check {
                    push(
                        format!(
                            "\"{}\" was applied {} ago and has been undone since, by an update \
                             or by the app itself.",
                            t.title,
                            ago(now - c.applied)
                        ),
                        Fix::Tweak(t.id),
                        (1, 0),
                    );
                    continue;
                }
                let measured = !t.targets(home).is_empty();
                if (measured && size.is_none_or(|s| s < t.suggest_from)) || (!measured && !tight) {
                    continue;
                }
                let cause = match size {
                    Some(s) if measured => {
                        format!("{}. {}", t.takes(s), t.why)
                    }
                    _ => t.why.to_string(),
                };
                let rank = size.map_or((2, 0), |s| (1, u64::MAX - s));
                push(cause, Fix::Tweak(t.id), rank);
            }
            Status::Pending => {
                let app = match t.after {
                    After::Approve(app) => app,
                    _ => "the app",
                };
                push(
                    format!("\"{}\" is waiting for your approval.", t.title),
                    Fix::Steps(format!(
                        "In System Settings → {}, install the MyDiskIsFull profile, then quit \
                         and reopen {app}.",
                        crate::profile::settings_place()
                    )),
                    (1, 0),
                );
            }
            Status::Applied => {
                if check.as_ref().is_some_and(|c| c.restart_pending) {
                    push(
                        format!("\"{}\" takes effect after a restart.", t.title),
                        Fix::Steps("Restart the Mac when it suits you.".into()),
                        (2, 0),
                    );
                } else if t.id == "chrome-ai-model" && size.is_some_and(|s| s >= GB) {
                    let dir = home.join(CHROME_MODEL);
                    push(
                        format!(
                            "Chrome no longer downloads its AI model, but the copy it has \
                             still takes {}.",
                            bytes(size.unwrap_or(0))
                        ),
                        Fix::Run {
                            command: format!("rm -rf {}", shell_quote(&dir)),
                            note: "Quit Chrome first. The policy keeps it from coming back.".into(),
                        },
                        (1, u64::MAX - size.unwrap_or(0)),
                    );
                }
            }
        }
    }
}

fn from_system(home: &Path, mount: &str, out: &mut Vec<Suggestion>) {
    let snaps = apfs::local_snapshots(Path::new(mount)).unwrap_or_default();
    // com.apple.TimeMachine.2026-10-18-204720.local; only a well-formed date
    // goes into the command.
    let dates: Vec<&str> = snaps
        .iter()
        .filter_map(|s| {
            s.strip_prefix("com.apple.TimeMachine.")?
                .strip_suffix(".local")
        })
        .filter(|d| {
            d.len() == 17
                && d.bytes().enumerate().all(|(i, b)| {
                    if matches!(i, 4 | 7 | 10) {
                        b == b'-'
                    } else {
                        b.is_ascii_digit()
                    }
                })
        })
        .collect();
    let when = |d: &str| match (d.get(..10), d.get(11..13), d.get(13..15)) {
        (Some(day), Some(h), Some(m)) => format!("{day} {h}:{m}"),
        _ => d.to_string(),
    };
    if !dates.is_empty() {
        out.push(Suggestion {
            cause: format!(
                "{} local Time Machine snapshot{} (from {}) keep{} the space of files deleted \
                 since {} taken, and the disk is nearly full.",
                dates.len(),
                if dates.len() == 1 { "" } else { "s" },
                when(dates[0]),
                if dates.len() == 1 { "s" } else { "" },
                if dates.len() == 1 {
                    "it was"
                } else {
                    "they were"
                },
            ),
            fix: Fix::Run {
                command: dates
                    .iter()
                    .map(|d| format!("tmutil deletelocalsnapshots {d}"))
                    .collect::<Vec<_>>()
                    .join("; "),
                note: "No password needed. Backups on your backup disk aren't touched; changes \
                       since the last backup lose their local copy."
                    .into(),
            },
            space: None,
            per_day: None,
            rank: (2, 0),
            key: "snapshots".into(),
        });
    }
    if let Ok(swap) = mdif_macos::sys::swap_used()
        && swap >= 2 * GB
    {
        out.push(Suggestion {
            cause: format!("macOS is using {} of swap, on the same disk.", bytes(swap)),
            fix: Fix::Steps(SWAP.into()),
            space: Some(swap),
            per_day: None,
            rank: (1, u64::MAX - swap),
            key: "swap".into(),
        });
    }
    let docker_disk = home.join("Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw");
    if let Some(size) = scan::size_of(&docker_disk).filter(|&s| s >= 10 * GB) {
        out.push(Suggestion {
            cause: format!(
                "Docker's disk image takes {}: images, containers and build cache, including \
                 ones no longer used.",
                bytes(size)
            ),
            fix: docker(),
            space: Some(size),
            per_day: None,
            rank: (2, 0),
            key: "docker".into(),
        });
    }
}

fn artifact(name: &str, project: &Path) -> Fix {
    let has = |f: &str| crate::privacy::may_open(project) && project.join(f).exists();
    let q = shell_quote(project);
    match name {
        "target" if has("Cargo.toml") => Fix::Run {
            command: format!(
                "cargo clean --manifest-path {}",
                shell_quote(&project.join("Cargo.toml"))
            ),
            note: "Rust rebuilds it locally, without downloading. Each worktree builds its own; \
                   pointing them at one folder (build.target-dir in .cargo/config.toml) stops \
                   that, but builds then wait for each other."
                .into(),
        },
        "build" | ".dart_tool" if has("pubspec.yaml") => Fix::Run {
            command: format!("cd {q} && flutter clean"),
            note: "Flutter rebuilds it locally on the next run.".into(),
        },
        "node_modules" => Fix::Steps(
            "Reinstalled from the lockfile when needed (that downloads). pnpm keeps one copy \
             of each package for every project instead of one per project."
                .into(),
        ),
        _ => Fix::Steps(format!(
            "{name} is build output: safe to delete while nothing is building, and it comes \
             back with the next build."
        )),
    }
}

fn docker() -> Fix {
    Fix::Run {
        command: "docker system prune".into(),
        note: "Removes stopped containers, unused networks, dangling images and build cache \
               (it asks first). Docker Desktop → Settings → Resources caps the disk image."
            .into(),
    }
}

/// One suggestion per fix: the same tweak or command for two causes is
/// listed once, under the first (best ranked) cause.
fn key_for(fix: &Fix, subject: &str) -> String {
    match fix {
        Fix::Tweak(id) => format!("tweak:{id}"),
        _ => subject.to_string(),
    }
}

fn display(path: &Path, home: &Path) -> String {
    let text = match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    };
    crate::text::printable(&text).into_owned()
}

/// `'…'`, safe to paste into a shell whatever the path holds.
fn shell_quote(path: &Path) -> String {
    let s = path.to_string_lossy();
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn ago(secs: i64) -> String {
    if secs >= 2 * DAY {
        format!("{} days", secs / DAY)
    } else if secs >= 2 * HOUR {
        format!("{} hours", secs / HOUR)
    } else {
        format!("{} minutes", (secs / 60).max(1))
    }
}

const CACHE: &str = "A cache: the app or macOS rebuilds it after it's cleared, so clearing it \
                     doesn't last. Leave it, unless the app offers a size limit.";
const SWAP: &str = "Restart the Mac when it suits you: swap files are only removed then. If it \
                    fills again, an app is using more memory than the Mac has.";
const WORKTREE: &str = "An agent's worktree, with its own build output. Once its work is \
                        merged, `git worktree remove <path>` deletes it (it refuses if \
                        anything is uncommitted).";
const MESSAGES: &str = "Messages keeps every attachment forever by default. Messages → Settings \
                        → General → Keep messages: 1 Year (or 30 Days) deletes older ones.";
const WHATSAPP: &str = "WhatsApp's chat backups. WhatsApp → Settings → Chats → Chat Backup: \
                        turn off Include Videos, or set Auto Backup to Off.";
const PHOTOS: &str = "With iCloud Photos on, Photos → Settings → iCloud → Optimize Mac Storage \
                      keeps full-size originals in iCloud and smaller versions here.";
const ICLOUD: &str = "System Settings → your name → iCloud → Drive: turn on Optimize Mac \
                      Storage, so files you don't use are kept in iCloud only.";
const XCODE: &str = "Xcode's DerivedData and old device support files are rebuilt when needed. \
                     Xcode → Settings → Locations shows DerivedData; delete it while Xcode \
                     isn't building.";

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/me";

    fn fix(p: &str) -> Fix {
        for_folder(Path::new(p), Path::new(HOME))
    }

    #[test]
    fn folders_map_to_the_fix_that_stops_them() {
        assert_eq!(fix("/Users/me/.gradle"), Fix::Tweak("gradle-caches"));
        assert_eq!(fix("/Users/me/.claude"), Fix::Tweak("claude-transcripts"));
        assert_eq!(
            fix("/Users/me/Library/Caches/Google"),
            Fix::Tweak("chrome-cache")
        );
        assert_eq!(fix("/Users/me/.Trash"), Fix::Tweak("trash-auto-empty"));
        assert!(matches!(
            fix("/Users/me/Library/Caches/com.spotify.client"),
            Fix::Leave(_)
        ));
        assert!(matches!(
            fix("/private/var/folders/vc/abc123/C/com.apple.mediaanalysisd"),
            Fix::Leave(_)
        ));
        let Fix::Steps(s) =
            fix("/Users/me/Library/Group Containers/group.net.whatsapp.WhatsApp.shared")
        else {
            panic!()
        };
        assert!(s.contains("Chat Backup"));
        let Fix::Steps(s) = fix("/Users/me/devel/app/.claude/worktrees/agent-1") else {
            panic!()
        };
        assert!(s.contains("git worktree remove"));
        let Fix::Steps(s) = fix("/Users/me/Library/CloudStorage/OneDrive-Personal") else {
            panic!()
        };
        assert!(s.starts_with("OneDrive-Personal keeps") && s.contains("Free Up Space"));
        assert!(matches!(
            fix("/Users/me/Library/Containers/com.docker.docker"),
            Fix::Run { .. }
        ));
        // Owners two levels under Library/Developer.
        assert!(matches!(
            fix("/Users/me/Library/Developer/CoreSimulator/Devices"),
            Fix::Run { .. }
        ));
        let Fix::Steps(s) = fix("/Users/me/Library/Developer/Xcode/DerivedData") else {
            panic!()
        };
        assert!(s.contains("DerivedData"));
        // A project named after an app is still a project.
        let Fix::Steps(s) = fix("/Users/me/devel/whatsapp-bot/node_modules") else {
            panic!()
        };
        assert!(s.contains("lockfile"), "{s}");
    }

    #[test]
    fn a_tweak_already_applied_is_not_suggested_again() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir(h.join(".claude")).unwrap();
        let tweak = Fix::Tweak("claude-transcripts");
        assert_eq!(settled(tweak.clone(), h), tweak);
        std::fs::write(
            h.join(".claude/settings.json"),
            "{\"cleanupPeriodDays\": 14}",
        )
        .unwrap();
        let Fix::Steps(s) = settled(tweak, h) else {
            panic!()
        };
        assert!(s.contains("already applied"), "{s}");
    }

    #[test]
    fn build_output_fixes_know_the_toolchain() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("it's a project");
        std::fs::create_dir(&proj).unwrap();
        std::fs::write(proj.join("Cargo.toml"), "").unwrap();
        let Fix::Run { command, .. } = for_folder(&proj.join("target"), Path::new(HOME)) else {
            panic!()
        };
        // Quoted for the shell, apostrophe included.
        assert!(
            command.starts_with("cargo clean --manifest-path '"),
            "{command}"
        );
        assert!(
            command.contains(r"it'\''s a project/Cargo.toml'"),
            "{command}"
        );
        // Without a manifest, `target` is just build output.
        std::fs::remove_file(proj.join("Cargo.toml")).unwrap();
        assert!(matches!(
            for_folder(&proj.join("target"), Path::new(HOME)),
            Fix::Steps(_)
        ));
    }

    #[test]
    fn churners_get_a_restart_and_unknown_names_stay_out_of_commands() {
        let Fix::Run { command, .. } = for_process("OneDrive", Path::new(HOME)) else {
            panic!()
        };
        assert!(command.contains("quit app \"OneDrive\"") && command.contains("open -a"));
        let Fix::Steps(s) = for_process("evil\"; rm -rf ~; \u{1b}[2J", Path::new(HOME)) else {
            panic!()
        };
        assert!(!s.contains('\u{1b}'), "{s:?}");
    }

    #[test]
    fn settings_churn_names_the_app_saving_its_settings() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let put = |rel: &str, size: usize, age: u64| {
            let p = h.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, vec![0u8; size]).unwrap();
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(age);
            std::fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(t)
                .unwrap();
        };
        put("Library/Preferences/com.example.small.plist", 2_000, 5);
        put("Library/Preferences/com.example.old.plist", 9_000_000, 3600);
        put(
            "Library/Containers/net.example.Chat/Data/Library/Preferences/net.example.Chat.plist",
            2_800_000,
            5,
        );
        let Fix::Steps(s) = for_process("cfprefsd", h) else {
            panic!()
        };
        assert!(
            s.contains("net.example.Chat") && s.contains("(3 MB)"),
            "{s}"
        );
        // Nothing big rewritten lately: how to find it.
        std::fs::remove_dir_all(h.join("Library/Containers")).unwrap();
        let Fix::Steps(s) = for_process("cfprefsd", h) else {
            panic!()
        };
        assert!(s.contains("ls -lt"), "{s}");
    }

    #[test]
    fn one_suggestion_per_fix() {
        let a = key_for(&Fix::Tweak("gradle-caches"), "/Users/me/.gradle");
        let b = key_for(&Fix::Tweak("gradle-caches"), "tweak probe");
        assert_eq!(a, b);
        assert_ne!(
            key_for(&Fix::Steps("x".into()), "/a"),
            key_for(&Fix::Steps("x".into()), "/b")
        );
    }

    #[test]
    fn every_fix_line_is_a_sentence() {
        for p in [
            "/Users/me/.gradle",
            "/Users/me/Library/Messages",
            "/Users/me/Library/Logs/SomeApp",
            "/Users/me/Library/Developer/CoreSimulator",
            "/private/var/vm",
            "/somewhere/else",
        ] {
            let line = fix(p).line();
            assert!(line.ends_with('.') || line.ends_with(')'), "{p}: {line}");
        }
        assert_eq!(
            Fix::Tweak("gradle-caches").line(),
            "Let Gradle clear caches unused for 14 days (`mdif tweaks apply gradle-caches`)."
        );
    }
}
