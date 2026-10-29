//! `mdif timemachine`: what goes into Time Machine's backups, what the local
//! snapshots hold and why, and what helps (see mdif_core::timemachine).
//! `exclude` and `include` keep folders a build or an install makes again
//! out of the backups or let them back in; `back-up` starts one now.

use std::path::{Path, PathBuf};

use anyhow::Result;
use mdif_core::store::Store;
use mdif_core::text::printable;
use mdif_core::timemachine::{self, Folder, Report};
use mdif_core::units::bytes;
use mdif_core::watch::unix_now;
use serde_json::json;

use crate::watch_cmd::{home, local, tilde};

/// `quick`: the backups and the snapshots only, without the folders.
pub fn show(db: &Path, quick: bool, json_out: bool) -> Result<()> {
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
    let done = timemachine::set_excluded(&paths, exclude, &keep);
    if json_out {
        let out = json!({
            "done": done.iter().filter(|(_, r)| r.is_ok()).map(|(p, _)| p).collect::<Vec<_>>(),
            "refused": done.iter().filter_map(|(p, r)| r.as_ref().err().map(|why| json!({"path": p, "why": why}))).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
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

pub fn back_up(json_out: bool) -> Result<()> {
    let r = timemachine::back_up_now();
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "started": r.is_ok(),
                "why": r.as_ref().err(),
            }))?
        );
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
        println!("Time Machine has no backup disk.");
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
        println!(
            "\nBackups are failing: {} tr{} since the last good one ({}) didn't finish. Until one \
             does, its snapshot keeps everything deleted or rewritten since{}.",
            b.failed,
            if b.failed == 1 { "y" } else { "ies" },
            b.since.map_or("never".into(), |t| local(t)
                .format("%b %-d %H:%M")
                .to_string()),
            if b.held > 0 {
                format!(": up to {}", bytes(b.held))
            } else {
                String::new()
            }
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
                    "goes into the next backup, but not the {} build folder{} in it",
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
    let backed: Vec<_> = r.rebuildable.iter().filter(|f| f.backed_up).collect();
    if !backed.is_empty() {
        let total: u64 = backed.iter().map(|f| f.bytes).sum();
        println!(
            "\nBacked up, though a build or an install makes them again ({}; `mdif timemachine \
             exclude <path>`):",
            bytes(total)
        );
        let (big, small): (Vec<&&Folder>, Vec<&&Folder>) =
            backed.iter().partition(|f| f.bytes >= 10_000_000);
        for f in &big {
            println!("  {:>9}  {}", bytes(f.bytes), printable(&tilde(&f.path)));
        }
        if !small.is_empty() {
            println!(
                "  {:>9}  in {} smaller ones",
                bytes(small.iter().map(|f| f.bytes).sum()),
                small.len()
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
            println!("  {}", printable(p));
        }
    }
}
