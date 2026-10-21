//! `mdif keep`: what MyDiskIsFull never suggests deleting, cleaning or
//! moving. The list is a text file next to the history (see
//! mdif_core::keep), which can be edited by hand too.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use mdif_core::keep::{self, Keep};
use mdif_core::privacy;
use mdif_core::text::printable;
use serde_json::json;

use crate::watch_cmd::{home, tilde};

/// Where the list is kept: next to the history.
pub fn file(db: &Path) -> PathBuf {
    db.with_file_name("keep.txt")
}

pub fn load(db: &Path) -> Result<Keep> {
    let f = file(db);
    Keep::load(&f, &home()).with_context(|| format!("reading {}", f.display()))
}

/// The list, or none when it can't be read: for what is shown anyway (an
/// incident and its fix), unlike suggestions, which wait for it.
pub fn load_or_warn(db: &Path) -> Keep {
    load(db).unwrap_or_else(|e| {
        eprintln!("The keep list is left out: {e:#}");
        Keep::default()
    })
}

pub fn list(db: &Path, json_out: bool) -> Result<()> {
    let home = home();
    // The app runs this unattended: a privacy prompt would wait for an
    // answer (see mdif_core::privacy).
    let probes = mdif_macos::sys::access_probes(&home);
    privacy::avoid_prompts(mdif_macos::sys::full_disk_access(&probes) != Some(true));
    let k = load(db)?;
    if json_out {
        let out: Vec<_> = k
            .entries()
            .map(|e| {
                json!({
                    "entry": e,
                    "path": keep::expand(e, &home),
                    "present": keep::present(e, &home),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if k.is_empty() {
        println!(
            "Nothing kept yet. `mdif keep add <path>` keeps a file or folder: nothing will \
             suggest deleting, cleaning or moving it."
        );
        return Ok(());
    }
    println!(
        "Never suggested for deleting, cleaning or moving ({}):",
        tilde(&file(db).to_string_lossy())
    );
    for e in k.entries() {
        let note = match keep::present(e, &home) {
            Some(0) => "  (nothing there now)".to_string(),
            Some(1) | None => String::new(),
            Some(n) => format!("  ({n} items)"),
        };
        println!("  {}{note}", printable(e));
    }
    Ok(())
}

pub fn add(db: &Path, paths: &[String], json_out: bool) -> Result<()> {
    let home = home();
    for p in paths {
        let entry = keep::entry(p, &home).with_context(|| format!("keeping {p}"))?;
        let added = keep::add(&file(db), &home, p)?;
        if !json_out {
            let verb = if added { "Kept" } else { "Already kept" };
            println!("{verb}: {}", printable(&entry));
        }
    }
    if json_out {
        list(db, true)?;
    }
    Ok(())
}

pub fn remove(db: &Path, paths: &[String], json_out: bool) -> Result<()> {
    let home = home();
    for p in paths {
        let entry = keep::entry(p, &home)?;
        if !keep::remove(&file(db), &home, p)? {
            bail!("{} isn't on the keep list (`mdif keep`)", printable(&entry));
        }
        if !json_out {
            println!("No longer kept: {}", printable(&entry));
        }
    }
    if json_out {
        list(db, true)?;
    }
    Ok(())
}
