//! `mdif timemachine`: what goes into Time Machine's backups, what the local
//! snapshots hold and why, and what helps (see mdif_core::timemachine).
//! `exclude` and `include` keep folders a build, an install or their tool
//! makes again out of the backups or let them back in; `back-up` starts one
//! now.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mdif_core::store::Store;
use mdif_core::text::printable;
use mdif_core::timemachine::{self, Folder, Report};
use mdif_core::units::bytes;
use mdif_core::watch::unix_now;
use serde_json::json;

use crate::watch_cmd::{home, local, tilde};

/// `quick`: the backups and the snapshots only, without the folders.
pub fn show(db: &Path, quick: bool, json_out: bool) -> Result<()> {
    // Its walks (what changed, what's rebuildable) never download a cloud
    // placeholder.
    mdif_core::os::fs::forbid_dataless_materialization()
        .context("disabling cloud downloads for this process")?;
    let r = if quick {
        timemachine::glance(&home(), unix_now())
    } else {
        let keep = crate::keep_cmd::load(db)?;
        // The history only adds what changed since the last backup.
        let store = db.exists().then(|| Store::open(db).ok()).flatten();
        // The big folders' sizes from the last whole-disk measurement.
        let measured = crate::space_cmd::load(&crate::space_cmd::file(db));
        timemachine::report(
            &home(),
            store.as_ref(),
            &keep,
            measured.as_ref(),
            unix_now(),
        )
    };
    if json_out {
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        print(&r);
    }
    Ok(())
}

pub fn set(db: &Path, paths: &[PathBuf], exclude: bool, json_out: bool) -> Result<()> {
    let keep = crate::keep_cmd::load(db)?;
    let paths: Vec<PathBuf> = paths
        .iter()
        .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()))
        .collect();
    let home = home();
    let paths = timemachine::expand(&paths, &home);
    let done = timemachine::set_excluded(&paths, exclude, &keep, &home);
    // What mdif keeps out is noted, so the page lists each even once it no
    // longer looks made again; what's let back in, no longer.
    let changed: Vec<PathBuf> = done
        .iter()
        .filter(|(_, r)| r.is_ok())
        .map(|(p, _)| p.clone())
        .collect();
    if !changed.is_empty() {
        let noted = Store::open(db)
            .and_then(|s| timemachine::remember_excluded(&s, &changed, exclude, &home));
        if let Err(e) = noted {
            eprintln!("noting what was kept out of Time Machine: {e:#}");
        }
    }
    if json_out {
        println!("{}", serde_json::to_string_pretty(&exclusion_value(&done))?);
        return Ok(());
    }
    for (p, r) in &done {
        let shown = printable(&tilde(&p.to_string_lossy())).into_owned();
        match r {
            Ok(()) if exclude => println!("Kept out of Time Machine: {shown}"),
            Ok(()) => println!("Backed up again: {shown}"),
            Err(why) => println!("Not changed: {}", printable(why)),
        }
    }
    Ok(())
}

/// What `mdif timemachine exclude|include --json` prints (the app's
/// `BackupExclusion`).
fn exclusion_value(done: &[(PathBuf, Result<(), String>)]) -> serde_json::Value {
    json!({
        "done": done.iter().filter(|(_, r)| r.is_ok()).map(|(p, _)| p).collect::<Vec<_>>(),
        "refused": done.iter().filter_map(|(p, r)| r.as_ref().err().map(|why| json!({"path": p, "why": why}))).collect::<Vec<_>>(),
    })
}

/// What `mdif timemachine back-up --json` prints (`BackupStarted`).
fn started_value(r: &Result<(), String>) -> serde_json::Value {
    json!({
        "started": r.is_ok(),
        "why": r.as_ref().err(),
    })
}

pub fn back_up(json_out: bool) -> Result<()> {
    let r = timemachine::back_up_now();
    if json_out {
        println!("{}", serde_json::to_string_pretty(&started_value(&r))?);
        return Ok(());
    }
    match r {
        Ok(()) => println!("Time Machine is backing up."),
        Err(why) => anyhow::bail!("{why}"),
    }
    Ok(())
}

fn print(r: &Report) {
    if !r.set_up {
        println!(
            "{}",
            if r.unreadable {
                "Time Machine's settings can't be read without Full Disk Access."
            } else {
                "Time Machine has no backup disk."
            }
        );
        return;
    }
    for d in &r.destinations {
        let server = d
            .server
            .as_deref()
            .map_or(String::new(), |s| format!(" on {}", printable(s)));
        let room = match (
            d.used,
            d.quota.or(d.available.zip(d.used).map(|(a, u)| a + u)),
        ) {
            (Some(u), Some(of)) => format!(": {} used of {}", bytes(u), bytes(of)),
            _ => String::new(),
        };
        println!("Backs up to {}{server}{room}.", printable(&d.name));
    }
    let every = match r.interval {
        i if i <= 3600 => "every hour".to_string(),
        86_400 => "every day".to_string(),
        604_800 => "every week".to_string(),
        i => format!("every {} hours", i / 3600),
    };
    println!(
        "{}, {every}. Last backup: {}.",
        if r.auto {
            "Automatically"
        } else {
            "By hand only"
        },
        r.last_backup.map_or("none".into(), |t| local(t)
            .format("%b %-d %H:%M")
            .to_string())
    );
    if let Some(run) = &r.running {
        println!(
            "Backing up now{}.",
            run.done
                .map_or(String::new(), |d| format!(": {:.0}%", d * 100.0))
        );
    }
    if let Some(b) = &r.backlog {
        let since = b.since.map_or("never".into(), |t| {
            local(t).format("%b %-d %H:%M").to_string()
        });
        let held = if b.held > 0 {
            format!(": up to {}", bytes(b.held))
        } else {
            String::new()
        };
        // Overdue (the Mac was closed, the disk not plugged in) isn't failing.
        let to =
            b.to.as_deref()
                .map_or(String::new(), |d| format!(" to \"{}\"", printable(d)));
        let what = if b.failed >= 2 || b.since.is_none() {
            format!(
                "Backups{to} are failing: {} tr{} since the last good one ({since}) didn't \
                 finish.",
                b.failed,
                if b.failed == 1 { "y" } else { "ies" },
            )
        } else {
            format!("No backup{to} has finished since {since}.")
        };
        println!(
            "\n{what} Until one does, its snapshot keeps everything deleted or rewritten \
             since{held}."
        );
    }
    if !r.tries.is_empty() {
        println!("\nLatest tries:");
        for t in &r.tries {
            let how = match t.finished {
                Some(f) if f != t.started => {
                    format!("finished {}", local(f).format("%H:%M"))
                }
                Some(_) => "finished".into(),
                None => "didn't finish".into(),
            };
            println!("  {}  {how}", local(t.started).format("%b %-d %H:%M"));
        }
    }
    println!(
        "\nLocal snapshots: {}{}",
        r.snapshots.len(),
        if r.held > 0 {
            format!(
                ", holding up to {} (what macOS counts as purgeable)",
                bytes(r.held)
            )
        } else {
            String::new()
        }
    );
    for s in &r.snapshots {
        println!(
            "  {}{}",
            local(s.taken).format("%b %-d %H:%M"),
            if s.reference {
                "  (the next backup starts from it)"
            } else {
                ""
            }
        );
    }
    if !r.changes.is_empty() {
        println!("\nChanged since the last backup:");
        for c in &r.changes {
            let sign = if c.bytes >= 0 { "+" } else { "-" };
            let state = match (c.backed_up, c.bytes >= 0) {
                (Some(false), _) => "not backed up".to_string(),
                (_, true) if c.kept_out > 0 => format!(
                    "goes into the next backup, but not the {} folder{} kept out in it",
                    c.kept_out,
                    if c.kept_out == 1 { "" } else { "s" }
                ),
                (_, true) => "goes into the next backup".to_string(),
                (_, false) => "held by the snapshots until then".to_string(),
            };
            println!(
                "  {sign}{:>9}  {}  ({state})",
                bytes(c.bytes.unsigned_abs()),
                printable(&tilde(&c.path))
            );
        }
    }
    let backed: Vec<&Folder> = r.rebuildable.iter().filter(|f| f.backed_up).collect();
    if !backed.is_empty() {
        let total: u64 = backed.iter().map(|f| f.bytes).sum();
        println!(
            "\nBacked up, though a build, an install or their tool makes them again ({} in {} \
             folders; `mdif timemachine exclude <folder, project or tool's folder>`):",
            bytes(total),
            backed.len()
        );
        let mut projects: BTreeMap<&str, Vec<&Folder>> = BTreeMap::new();
        for f in &backed {
            projects.entry(&f.project).or_default().push(f);
        }
        let mut projects: Vec<(&str, u64, Vec<&Folder>)> = projects
            .into_iter()
            .map(|(p, fs)| (p, fs.iter().map(|f| f.bytes).sum(), fs))
            .collect();
        projects.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let (big, small): (Vec<_>, Vec<_>) = projects
            .iter()
            .partition(|(_, size, _)| *size >= 10_000_000);
        for (project, size, folders) in &big {
            match folders.as_slice() {
                [f] => println!("  {:>9}  {}", bytes(f.bytes), printable(&tilde(&f.path))),
                _ => println!(
                    "  {:>9}  {}  ({} folders)",
                    bytes(*size),
                    printable(&tilde(project)),
                    folders.len()
                ),
            }
        }
        if !small.is_empty() {
            println!(
                "  {:>9}  in {} smaller project{}",
                bytes(small.iter().map(|(_, size, _)| size).sum()),
                small.len(),
                if small.len() == 1 { "" } else { "s" }
            );
        }
    }
    let out = r.rebuildable.len() - backed.len();
    if out > 0 {
        println!("\n{out} more like them are kept out already.");
    }
    if !r.skipped.is_empty() {
        println!("\nLeft out in Time Machine's settings:");
        for p in &r.skipped {
            println!("  {}", printable(&tilde(p)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(v: &serde_json::Value) -> Vec<String> {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    }

    /// The fixtures the app's contract check decodes for what a fresh
    /// history can't show: a folder kept out, and a backup started.
    #[test]
    fn the_apps_time_machine_fixtures_have_the_shape_printed() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../app/Tests/exclude.json")).unwrap();
        let real = exclusion_value(&[
            (PathBuf::from("/x"), Ok(())),
            (PathBuf::from("/y"), Err(String::new())),
        ]);
        assert_eq!(keys(&fixture), keys(&real));
        assert!(!fixture["done"].as_array().unwrap().is_empty());
        for r in fixture["refused"].as_array().unwrap() {
            assert_eq!(keys(r), keys(&real["refused"][0]));
        }
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../app/Tests/back-up.json")).unwrap();
        assert_eq!(keys(&fixture), keys(&started_value(&Err(String::new()))));
        assert_eq!(keys(&fixture), keys(&started_value(&Ok(()))));
    }
}
