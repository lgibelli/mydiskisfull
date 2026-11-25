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
    let measured_now = fresh.is_none();
    let mut m = match fresh {
        Some(m) => m,
        None => {
            let started = unix_now();
            let mut m = space::measure(&home())?;
            // What was cleaned up while it measured (and patched into the
            // kept one meanwhile) may have been listed before it went: it's
            // measured again in this one too, before this one is kept.
            let saved = locked(db, || {
                let since = patched_since(db, started);
                if !since.is_empty() {
                    space::update(&mut m, &since)?;
                }
                save(&kept, &m)?;
                // This one has everything patched until now.
                let _ = std::fs::remove_file(journal(db));
                Ok(())
            });
            if let Err(e) = saved {
                eprintln!("The measurement isn't kept for next time: {e:#}");
            }
            m
        }
    };
    // Which simulators can be erased goes by how they are now: a fraction
    // of a second, and only with Xcode's simulators on the disk.
    if m.home.join(mdif_core::xcode::DEVICES).is_dir() {
        m.simulators = mdif_core::xcode::simulators();
    }
    // Which system images Android's emulators use, the same way: their
    // settings, a few small files.
    m.avds = mdif_core::android::avds(&m.home).ok();
    // The snapshots as they are now, not as they were at the scan: Time
    // Machine takes and deletes them by the hour.
    let snapshots =
        mdif_core::apfs::local_snapshots(Path::new(mdif_core::map::DATA_ROOT)).unwrap_or_default();
    let r =
        space::report(&m, &keep, &snapshots).with_update(mdif_core::os_update::staged().as_ref());
    // Into the history when measured now. One kept from before is there
    // already, at its time, and each clean-up patched into it since at
    // the clean-up's: at its time again, it would say the disk was made
    // then of what it was after them.
    if measured_now {
        record(db, r.measured, &r);
    }
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
    let patched = locked(db, || {
        // Said for a measure under way, which saves after this.
        note_patched(db, paths)?;
        let Some(mut m) = load(&kept) else {
            return Ok(None);
        };
        space::update(&mut m, paths)?;
        save(&kept, &m)?;
        Ok(Some(m))
    });
    let m = match patched {
        Ok(Some(m)) => m,
        Ok(None) => return,
        Err(e) => {
            eprintln!("The breakdown is measured again only on Refresh: {e:#}");
            return;
        }
    };
    if let Ok(keep) = crate::keep_cmd::load(db) {
        // What the disk is made of doesn't depend on the snapshots.
        record(db, unix_now(), &space::report(&m, &keep, &[]));
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

/// Holds the lock on the kept measurement while `f` runs: a patch and a
/// measure's save one after the other, never interleaved.
fn locked<T>(db: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let dir = db.parent().context("no folder for it")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(db.with_file_name("space.lock"))?;
    mdif_macos::fs::lock(&lock)?;
    f()
}

/// What clean-ups patched into the kept measurement, and when: a line
/// each, `<unix seconds> <path>`, until a measure that saw them is kept.
fn journal(db: &Path) -> PathBuf {
    db.with_file_name("space.patched")
}

fn note_patched(db: &Path, paths: &[PathBuf]) -> Result<()> {
    use std::io::Write;
    let now = unix_now();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(journal(db))?;
    for p in paths {
        // A path with a line break in it can't be noted: it's measured
        // again with the next measure only.
        if let Some(p) = p.to_str().filter(|p| !p.contains('\n')) {
            writeln!(f, "{now} {p}")?;
        }
    }
    Ok(())
}

/// The paths patched at `since` or after.
fn patched_since(db: &Path, since: i64) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(journal(db)) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = text
        .lines()
        .filter_map(|l| l.split_once(' '))
        .filter(|(t, _)| t.parse::<i64>().is_ok_and(|t| t >= since))
        .map(|(_, p)| PathBuf::from(p))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// A kept measurement, if there is one this version can read.
pub(crate) fn load(file: &Path) -> Option<Measured> {
    let text = std::fs::read(file).ok()?;
    serde_json::from_slice::<Measured>(&text)
        .ok()
        .filter(|m| m.version == space::VERSION)
}

/// Written whole, then moved into place, so a reader never sees half of it.
fn save(file: &Path, m: &Measured) -> Result<()> {
    let dir = file.parent().context("no folder for it")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    // Its own temporary file: two writers never write into one.
    let tmp = file.with_extension(format!("tmp.{}", std::process::id()));
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
    if r.partial {
        println!(
            "A clean-up since was in a folder too big to measure again in a moment: \
             `mdif space --refresh` shows it."
        );
    }
    if !r.comes_back.is_empty() {
        let back: Vec<String> = r
            .comes_back
            .iter()
            .map(|c| {
                let what = match c.what {
                    "trash" => "in the Trash",
                    "update" => "macOS update",
                    w => w,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_patched_while_it_measured_is_measured_again_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("history.db");
        let started = unix_now();
        assert!(patched_since(&db, started).is_empty());
        let derived = PathBuf::from("/Users/me/Library/Developer/Xcode/DerivedData");
        let spaced = PathBuf::from("/Users/me/devel/my app/target");
        let broken = PathBuf::from("/Users/me/a\nb");
        locked(&db, || {
            note_patched(&db, &[derived.clone(), spaced.clone(), broken])
        })
        .unwrap();
        locked(&db, || note_patched(&db, std::slice::from_ref(&derived))).unwrap();
        // Once each, spaces kept; a name with a line break can't be noted.
        assert_eq!(patched_since(&db, started), [derived, spaced]);
        // Patched before it started: it saw that already.
        assert!(patched_since(&db, unix_now() + 60).is_empty());
    }
}
