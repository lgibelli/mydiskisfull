//! `mdif tidy`: the clean-ups the watcher does once a day, each only once
//! it's turned on (see mdif_core::tidy). The switches and what each did
//! last are kept in the history.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use mdif_core::keep::Keep;
use mdif_core::privacy;
use mdif_core::store::Store;
use mdif_core::text::printable;
use mdif_core::tidy::{self, Id, Outcome, Tidy};
use mdif_core::units::bytes;
use mdif_core::watch::unix_now;
use serde_json::{Value, json};

use crate::watch_cmd::{home, local, tilde};

pub fn list(db: &Path, json_out: bool) -> Result<()> {
    let store = Store::open(db)?;
    let tidies = tidy::list(&store)?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&list_value(&tidies))?);
        return Ok(());
    }
    println!("Tidy-ups the watcher does once a day, once you turn them on (`mdif tidy on <id>`):");
    for t in &tidies {
        println!();
        let state = match (t.available, t.on) {
            (false, _) => "n/a",
            (true, true) => "on",
            (true, false) => "off",
        };
        println!("  {:<12} {:<4} {}", t.id, state, t.title);
        println!("  {:<17} {}", "", t.detail);
        if let Some(why) = &t.unavailable_why {
            println!("  {:<17} {why}", "");
        }
        if let Some(l) = &t.last {
            let freed = if l.freed > 0 {
                format!(" ({} freed)", bytes(l.freed))
            } else {
                String::new()
            };
            println!(
                "  {:<17} Last run {}: {}{freed}",
                "",
                local(l.at).format("%b %-d %H:%M"),
                printable(l.note.as_deref().unwrap_or("done"))
            );
        }
    }
    Ok(())
}

/// What `mdif tidy --json` prints.
fn list_value(tidies: &[Tidy]) -> Value {
    json!({ "tidies": tidies })
}

pub fn set(db: &Path, id: &str, on: bool, json_out: bool) -> Result<()> {
    let id = parse(id)?;
    let store = Store::open(db)?;
    tidy::set_on(&store, id, on)?;
    if !json_out {
        let how = if on {
            "On: the watcher does it once a day"
        } else {
            "Off"
        };
        println!("{how}: {} ({}).", id.title(), id.id());
        if let Some(why) = id.unavailable().filter(|_| on) {
            println!("Not on this Mac, though: {why}");
        }
        println!();
    }
    list(db, json_out)
}

fn parse(id: &str) -> Result<Id> {
    match Id::parse(id) {
        Some(id) => Ok(id),
        None => {
            let ids: Vec<&str> = Id::ALL.iter().map(|i| i.id()).collect();
            bail!("no tidy-up called {}: {}", printable(id), ids.join(", "))
        }
    }
}

/// Runs the tidy-ups named, or all that are on; records what each did,
/// except on a dry run.
pub fn run(db: &Path, ids: &[String], dry_run: bool, json_out: bool) -> Result<()> {
    let store = Store::open(db)?;
    let ids: Vec<Id> = if ids.is_empty() {
        let mut on = Vec::new();
        for id in Id::ALL {
            if tidy::is_on(&store, id)? {
                on.push(id);
            }
        }
        on
    } else {
        ids.iter().map(|i| parse(i)).collect::<Result<_>>()?
    };
    let home = home();
    // The app runs this too: a privacy prompt would wait for an answer
    // (see mdif_core::privacy).
    let probes = mdif_macos::sys::access_probes(&home);
    privacy::avoid_prompts(mdif_macos::sys::full_disk_access(&probes) != Some(true));
    // Nothing is deleted without knowing what you keep.
    let keep = crate::keep_cmd::load(db)?;
    let now = unix_now();
    let mut ran: Vec<(Id, Outcome)> = Vec::new();
    for id in ids {
        if let Some(why) = id.unavailable() {
            let mut o = Outcome::default();
            o.failed.push(tidy::Failed {
                id: id.id(),
                path: Default::default(),
                why,
            });
            ran.push((id, o));
            continue;
        }
        let o = tidy::run(id, &home, &keep, now, dry_run);
        if !dry_run {
            tidy::record(&store, id, &o.last(now))?;
            cleaned(&store, db, now, &o);
        }
        ran.push((id, o));
    }

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&run_value(dry_run, &ran))?
        );
        return Ok(());
    }
    if ran.is_empty() {
        println!(
            "No tidy-up is on. `mdif tidy on <id>` turns one on, or name it: `mdif tidy run \
             caches --dry-run`."
        );
        return Ok(());
    }
    let freed: u64 = ran.iter().map(|(_, o)| o.freed()).sum();
    let out = ran
        .iter()
        .flat_map(|(_, o)| &o.done)
        .filter(|d| d.what == "excluded")
        .count();
    let frees = ran.iter().any(|(id, _)| *id != Id::Backups);
    for (id, o) in &ran {
        print_outcome(*id, o, dry_run);
    }
    println!();
    let folders = |n: usize| format!("{n} folder{}", if n == 1 { "" } else { "s" });
    let mut said = Vec::new();
    match (dry_run, frees) {
        (true, true) => said.push(format!("That would free about {}.", bytes(freed))),
        (false, true) => said.push(format!("Freed {}.", bytes(freed))),
        _ => {}
    }
    match (dry_run, out) {
        (_, 0) => {}
        (true, n) => said.push(format!("{} would be kept out of Time Machine.", folders(n))),
        (false, n) => said.push(format!("Kept {} out of Time Machine.", folders(n))),
    }
    if dry_run {
        said.push("Nothing was changed.".into());
    }
    if !said.is_empty() {
        println!("{}", said.join(" "));
    }
    Ok(())
}

fn print_outcome(id: Id, o: &Outcome, dry_run: bool) {
    println!();
    println!("{} ({}):", id.title(), id.id());
    let shown = |p: &Path| printable(&tilde(&p.to_string_lossy())).into_owned();
    for d in &o.done {
        let verb = match (dry_run, d.what) {
            (true, "emptied") => "Would empty",
            (true, "excluded") => "Would keep out of Time Machine",
            (true, _) => "Would delete",
            (false, "emptied") => "Emptied",
            (false, "excluded") => "Kept out of Time Machine",
            (false, _) => "Deleted",
        };
        if d.what == "excluded" {
            println!("  {verb}: {}", shown(&d.path));
        } else {
            println!("  {verb}: {} ({})", shown(&d.path), bytes(d.bytes));
        }
    }
    for (path, why) in &o.skipped {
        println!("  Left alone: {} ({})", shown(path), printable(why));
    }
    for f in &o.failed {
        println!("  Not done: {}", printable(&f.why));
    }
    if o.done.is_empty() && o.failed.is_empty() {
        println!("  Nothing to tidy.");
    }
}

/// What `mdif tidy run --json` prints.
fn run_value(dry_run: bool, ran: &[(Id, Outcome)]) -> Value {
    json!({
        "dry_run": dry_run,
        "freed": ran.iter().map(|(_, o)| o.freed()).sum::<u64>(),
        "done": ran.iter().flat_map(|(_, o)| &o.done).collect::<Vec<_>>(),
        "failed": ran.iter().flat_map(|(_, o)| &o.failed).collect::<Vec<_>>(),
    })
}

/// For the watcher, after a sample: runs the tidy-ups that are due (on, and
/// not run for a day), in the background state, and records what each did.
/// What it deleted goes to the log, as do errors; none is fatal.
pub fn run_due(store: &Store, db: &Path, now: i64) {
    let stamp = || local(unix_now()).format("%F %H:%M");
    let due = match tidy::due(store, now) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{} tidy-ups: {e:#}", stamp());
            return;
        }
    };
    let due: Vec<Id> = due
        .into_iter()
        .filter(|id| id.unavailable().is_none())
        .collect();
    if due.is_empty() {
        return;
    }
    let keep: Keep = match crate::keep_cmd::load(db) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("{} tidy-ups wait: {e:#}", stamp());
            return;
        }
    };
    let _background = mdif_macos::sys::background()
        .inspect_err(|e| eprintln!("{} tidy-ups at normal priority: {e}", stamp()));
    let home = home();
    for id in due {
        let o = tidy::run(id, &home, &keep, now, false);
        for d in &o.done {
            eprintln!(
                "{} tidy {}: {} {} ({})",
                stamp(),
                id.id(),
                d.what,
                printable(&tilde(&d.path.to_string_lossy())),
                bytes(d.bytes)
            );
        }
        for f in &o.failed {
            eprintln!(
                "{} tidy {}: not done: {}",
                stamp(),
                id.id(),
                printable(&f.why)
            );
        }
        if let Err(e) = tidy::record(store, id, &o.last(now)) {
            eprintln!("{} tidy {}: {e:#}", stamp(), id.id());
        }
        cleaned(store, db, now, &o);
    }
}

/// What a tidy-up deleted or emptied, into the history (the forecast leaves
/// it out) and the kept measurement. Folders only kept out of Time Machine
/// changed neither.
fn cleaned(store: &Store, db: &Path, start: i64, o: &Outcome) {
    let changed: Vec<PathBuf> = o
        .done
        .iter()
        .filter(|d| d.what != "excluded")
        .map(|d| d.path.clone())
        .collect();
    if !changed.is_empty() {
        crate::watch_cmd::record_cleanup(store, start, o.freed());
        crate::space_cmd::patch(db, &changed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn keys(v: &Value) -> BTreeSet<String> {
        v.as_object().unwrap().keys().cloned().collect()
    }

    /// The fixture the app's contract check decodes (app/Tests/tidy.json),
    /// held to what `mdif tidy --json` prints: the same keys at every level.
    #[test]
    fn the_apps_tidy_fixture_has_the_shape_tidy_prints() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../app/Tests/tidy.json")).unwrap();
        let store = Store::open_in_memory().unwrap();
        tidy::record(
            &store,
            Id::Caches,
            &tidy::Last {
                at: 1,
                freed: 1,
                done: 1,
                failed: 0,
                note: Some(String::new()),
            },
        )
        .unwrap();
        let real = list_value(&tidy::list(&store).unwrap());
        assert_eq!(keys(&fixture), keys(&real));
        // Caches, the one with a last run.
        let real_tidy = &real["tidies"][Id::ALL.iter().position(|&i| i == Id::Caches).unwrap()];
        let mut ids = Vec::new();
        let mut lasts = 0;
        for t in fixture["tidies"].as_array().unwrap() {
            assert_eq!(keys(t), keys(real_tidy));
            ids.push(t["id"].as_str().unwrap());
            if !t["last"].is_null() {
                assert_eq!(keys(&t["last"]), keys(&real_tidy["last"]));
                lasts += 1;
            }
            assert_eq!(
                t["available"].as_bool().unwrap(),
                t["unavailable_why"].is_null()
            );
        }
        let real_ids: Vec<&str> = Id::ALL.iter().map(|i| i.id()).collect();
        assert_eq!(ids, real_ids, "all of them, in order");
        assert!(lasts > 0, "the fixture shows a last run");
    }

    /// The same for what a run prints (app/Tests/tidy-run.json).
    #[test]
    fn the_apps_tidy_run_fixture_has_the_shape_a_run_prints() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../app/Tests/tidy-run.json")).unwrap();
        let mut o = Outcome::default();
        o.done.push(tidy::Done {
            id: "caches",
            path: PathBuf::from("/x"),
            what: "emptied",
            bytes: 1,
        });
        o.failed.push(tidy::Failed {
            id: "caches",
            path: PathBuf::from("/y"),
            why: String::new(),
        });
        let real = run_value(false, &[(Id::Caches, o)]);
        assert_eq!(keys(&fixture), keys(&real));
        for d in fixture["done"].as_array().unwrap() {
            assert_eq!(keys(d), keys(&real["done"][0]));
        }
        for f in fixture["failed"].as_array().unwrap() {
            assert_eq!(keys(f), keys(&real["failed"][0]));
        }
        assert!(!fixture["done"].as_array().unwrap().is_empty());
        assert!(!fixture["failed"].as_array().unwrap().is_empty());
        let freed: u64 = fixture["done"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["bytes"].as_u64().unwrap())
            .sum();
        assert_eq!(fixture["freed"].as_u64(), Some(freed));
    }
}
