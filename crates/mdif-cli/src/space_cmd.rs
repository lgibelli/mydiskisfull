//! `mdif space`: where the space goes, sorted into kinds, biggest first, and
//! what of it is waste (see mdif_core::space). Scanning takes as long as the
//! disk map, so the measurement is kept next to the history (`space.json`)
//! and shown again while it's younger than `--max-age`, with what you keep
//! now. What the disk was made of goes into the history too (`strata`), for
//! the app's chart of it over time.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mdif_core::space::{self, Measured, Report};
use mdif_core::text::printable;
use mdif_core::units::bytes;
use mdif_core::watch::unix_now;

use crate::watch_cmd::{home, tilde};

/// Where the measurement is kept: next to the history.
pub fn file(db: &Path) -> PathBuf {
    db.with_file_name("space.json")
}

pub fn space(db: &Path, json_out: bool, max_age: Option<u64>, refresh: bool) -> Result<()> {
    let keep = crate::keep_cmd::load(db)?;
    let kept = file(db);
    let now = unix_now();
    let fresh = match max_age {
        Some(age) if !refresh => load(&kept).filter(|m| now - m.measured <= age as i64),
        _ => None,
    };
    let m = match fresh {
        Some(m) => m,
        None => {
            let m = space::measure(&home())?;
            if let Err(e) = save(&kept, &m) {
                eprintln!("The measurement isn't kept for next time: {e:#}");
            }
            m
        }
    };
    let r = space::report(&m, &keep);
    // Again for the same measurement replaces it: nothing piles up.
    record(db, r.measured, &r);
    if json_out {
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        print(&r, now);
    }
    Ok(())
}

/// After a clean-up: the kept measurement patched where `paths` changed
/// ([`space::update`]), so the app's breakdown follows at once. Nothing kept,
/// nothing to do; a failure is said and left for Refresh.
pub fn patch(db: &Path, paths: &[PathBuf]) {
    let kept = file(db);
    let Some(mut m) = load(&kept) else {
        return;
    };
    let done = space::update(&mut m, paths).and_then(|()| save(&kept, &m));
    if let Err(e) = done {
        eprintln!("The breakdown is measured again only on Refresh: {e:#}");
        return;
    }
    if let Ok(keep) = crate::keep_cmd::load(db) {
        record(db, unix_now(), &space::report(&m, &keep));
    }
}

/// What the disk is made of, into the history, if there is one (the
/// watcher makes it).
fn record(db: &Path, ts: i64, r: &Report) {
    if !db.exists() {
        return;
    }
    let done = mdif_core::store::Store::open(db).and_then(|s| s.add_strata(ts, &space::strata(r)));
    if let Err(e) = done {
        eprintln!("What the disk is made of isn't kept in the history: {e:#}");
    }
}

/// A kept measurement, if there is one this version can read.
fn load(file: &Path) -> Option<Measured> {
    let text = std::fs::read(file).ok()?;
    serde_json::from_slice::<Measured>(&text)
        .ok()
        .filter(|m| m.version == space::VERSION)
}

/// Written whole, then moved into place, so a reader never sees half of it.
fn save(file: &Path, m: &Measured) -> Result<()> {
    let dir = file.parent().context("no folder for it")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = file.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(m)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, file).with_context(|| format!("writing {}", file.display()))?;
    Ok(())
}

fn print(r: &Report, now: i64) {
    let v = &r.volume;
    println!(
        "{} used of {}, {} free. Measured {} ago, in {:.0} s.",
        bytes(v.used),
        bytes(v.total),
        bytes(v.free),
        ago(now - r.measured),
        r.took_secs
    );
    if !r.comes_back.is_empty() {
        let back: Vec<String> = r
            .comes_back
            .iter()
            .map(|c| {
                let what = if c.what == "trash" {
                    "in the Trash"
                } else {
                    c.what
                };
                format!("{} {what} ({})", bytes(c.bytes), c.how)
            })
            .collect();
        println!("Of what's used: {}.", back.join(" · "));
    }
    println!();
    println!(
        "{:>10} {:>10} {:>10}  KIND, AND ITS BIGGEST",
        "SIZE", "FREES", "WASTE"
    );
    for k in &r.kinds {
        let waste = if k.reclaimable > 0 {
            bytes(k.reclaimable)
        } else {
            String::new()
        };
        println!(
            "{:>10} {:>10} {:>10}  {}",
            bytes(k.bytes),
            bytes(k.frees),
            waste,
            k.title
        );
        for i in k.items.iter().take(3) {
            let mut tags = i.flags.join(", ");
            if !tags.is_empty() {
                tags = format!(" [{tags}]");
            }
            let note = i
                .note
                .as_deref()
                .map_or(String::new(), |n| format!(" — {n}"));
            let place = if i.path.is_empty() || i.path.ends_with(&*i.name) {
                String::new()
            } else {
                format!("  {}", tilde(&i.path))
            };
            println!(
                "{:>10} {:>10} {:>10}    {}{}{}{}",
                bytes(i.bytes),
                "",
                "",
                printable(&i.name),
                printable(&tags),
                printable(&note),
                printable(&place)
            );
        }
    }
    println!();
    println!(
        "SIZE = on disk (clones counted once). FREES = freed if deleted. WASTE = unused apps,"
    );
    println!("build output, caches and files untouched for a year, not counting what you keep.");
}

fn ago(secs: i64) -> String {
    match secs.max(0) {
        s if s < 90 => format!("{s} s"),
        s if s < 90 * 60 => format!("{} min", s / 60),
        s if s < 36 * 3600 => format!("{} h", s / 3600),
        s => format!("{} days", s / 86_400),
    }
}
