//! `mdif clean`: the clean-ups a suggestion offers, done when asked (see
//! mdif_core::clean). Each path is checked again first, and refused unless
//! it is still what was suggested; the rest go ahead.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use mdif_core::clean::{self, Done, Step};
use mdif_core::text::printable;
use mdif_core::units::bytes;
use mdif_core::watch::unix_now;
use mdif_core::{scan, worktrees};
use serde_json::json;

use crate::watch_cmd::{home, tilde};

pub fn clean(
    db: &Path,
    volume: &Path,
    paths: &[PathBuf],
    dry_run: bool,
    json_out: bool,
) -> Result<()> {
    let home = home();
    let keep = crate::keep_cmd::load(db)?;
    let paths: Vec<PathBuf> = paths
        .iter()
        .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()))
        .collect();
    // Only worth running git when one of them is a worktree.
    let finished = if paths.iter().any(|p| p.join(".git").is_file()) {
        worktrees::finished(&home, unix_now())
    } else {
        Vec::new()
    };
    // Only worth reading the history when one of them is an app.
    let churning = if paths
        .iter()
        .any(|p| p.extension().is_some_and(|e| e == "app"))
    {
        churning(db)
    } else {
        Vec::new()
    };
    let mut steps: Vec<Step> = Vec::new();
    let mut failed: Vec<(PathBuf, String)> = Vec::new();
    for p in &paths {
        match clean::plan(p, &home, &keep, &finished, &churning) {
            Ok(s) => steps.push(s),
            Err(why) => failed.push((p.clone(), why)),
        }
    }

    if dry_run {
        if json_out {
            let out = json!({
                "would": steps.iter().map(|s| json!({
                    "path": s.path(),
                    "how": how(s),
                })).collect::<Vec<_>>(),
                "refused": failed.iter().map(|(p, why)| json!({"path": p, "why": why})).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
        } else {
            for s in &steps {
                let verb = match s {
                    Step::RemoveWorktree(_) => "Would remove (git worktree remove)",
                    Step::Trash { .. } => "Would move to the Trash",
                    Step::Delete(_) => "Would delete",
                    Step::Empty { .. } => "Would empty",
                    Step::EmptyTrash(_) => "Would empty the Trash",
                    Step::FinderTrash(_) => "Would move to the Trash with Finder",
                    Step::Restart { .. } => "Would restart a helper of",
                };
                println!("{verb}: {}", printable(&tilde(&s.path().to_string_lossy())));
            }
            for (_, why) in &failed {
                println!("Refused: {}", printable(why));
            }
        }
        return Ok(());
    }

    let before = mdif_macos::fs::statfs(volume)?.avail;
    let mut done: Vec<(Done, Option<u64>)> = Vec::new();
    // What goes through Finder goes at once: one password, not one each.
    let (finder, steps): (Vec<Step>, Vec<Step>) = steps
        .into_iter()
        .partition(|s| matches!(s, Step::FinderTrash(_)));
    if !finder.is_empty() {
        let paths: Vec<PathBuf> = finder.iter().map(|s| s.path().to_path_buf()).collect();
        match clean::finder_trash(&paths) {
            Ok(()) => done.extend(paths.into_iter().map(|path| {
                (
                    Done {
                        path,
                        how: "trashed",
                        to: None,
                    },
                    None,
                )
            })),
            Err(why) => failed.extend(paths.into_iter().map(|p| (p, why.clone()))),
        }
    }
    for s in &steps {
        // What goes to the Trash still takes its space there.
        let size = matches!(s, Step::Trash { .. })
            .then(|| scan::size_of(s.path()))
            .flatten();
        match clean::run(s, &home) {
            Ok(d) => {
                if let Step::Restart { helper, .. } = s {
                    restarted(db, helper);
                }
                done.push((d, size))
            }
            Err(why) => failed.push((s.path().to_path_buf(), why)),
        }
    }
    let freed = mdif_macos::fs::statfs(volume)?.avail.saturating_sub(before);
    if !done.is_empty() {
        crate::watch_cmd::record_clean(db, freed);
        // What went to the Trash is there now too.
        let mut changed: Vec<PathBuf> = done.iter().map(|(d, _)| d.path.clone()).collect();
        if done.iter().any(|(d, _)| d.how == "trashed") {
            changed.push(home.join(".Trash"));
        }
        crate::space_cmd::patch(db, &changed);
    }
    let in_trash: u64 = done.iter().filter_map(|(_, s)| *s).sum();

    if json_out {
        let out = json!({
            "done": done.iter().map(|(d, size)| json!({
                "path": d.path,
                "how": d.how,
                "to": d.to,
                "size": size,
            })).collect::<Vec<_>>(),
            "failed": failed.iter().map(|(p, why)| json!({"path": p, "why": why})).collect::<Vec<_>>(),
            "freed": freed,
            "in_trash": in_trash,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        for (d, size) in &done {
            let shown = printable(&tilde(&d.path.to_string_lossy())).into_owned();
            match d.how {
                "trashed" => println!(
                    "Moved to the Trash: {shown}{}",
                    size.map_or(String::new(), |s| format!(
                        " ({} comes back once the Trash is emptied)",
                        bytes(s)
                    ))
                ),
                "deleted" => println!("Deleted: {shown}"),
                "emptied" => println!("Emptied: {shown}"),
                "erased" => println!("Emptied the Trash"),
                "restarted" => println!("Restarted a helper of {shown}"),
                _ => println!("Removed: {shown} (its branch stays)"),
            }
        }
        for (_, why) in &failed {
            println!("Not done: {}", printable(why));
        }
        if !done.is_empty() {
            println!("The disk has {} more free.", bytes(freed));
        }
    }
    // With --json the reasons are in what was printed.
    if !json_out && !failed.is_empty() && done.is_empty() {
        bail!("nothing was cleaned");
    }
    Ok(())
}

fn how(s: &Step) -> &'static str {
    match s {
        Step::RemoveWorktree(_) => "remove",
        Step::Trash { .. } => "trash",
        Step::Delete(_) => "delete",
        Step::Empty { .. } => "empty",
        Step::EmptyTrash(_) => "empty-trash",
        Step::FinderTrash(_) => "finder-trash",
        Step::Restart { .. } => "restart",
    }
}

/// When a helper was restarted, so the suggestion can wait to see whether
/// that worked before offering it again (see mdif_core::suggest).
fn restarted(db: &Path, helper: &str) {
    if let Ok(store) = mdif_core::store::Store::open(db) {
        let _ = store.set_meta(
            &mdif_core::suggest::restarted_key(helper),
            &unix_now().to_string(),
        );
    }
}

/// The processes that keep rewriting the disk now: open churn incidents.
fn churning(db: &Path) -> Vec<String> {
    let Ok(store) = mdif_core::store::Store::open(db) else {
        return Vec::new();
    };
    store
        .incidents_since(unix_now())
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.closed.is_none() && e.incident.kind == mdif_core::incidents::Kind::Churn)
        .map(|e| e.incident.subject)
        .collect()
}
