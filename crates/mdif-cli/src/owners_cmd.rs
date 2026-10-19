//! `mdif owners` (which folders grow, and what comes back after a cleanup)
//! and `mdif incidents` (runaway fillers).

use std::path::Path;

use anyhow::Result;
use mdif_core::forecast::{DAY, HOUR};
use mdif_core::owners;
use mdif_core::store::Store;
use mdif_core::suggest;
use mdif_core::text::printable;
use mdif_core::units::bytes;
use mdif_core::watch::unix_now;
use serde_json::json;

use crate::tweaks_cmd;
use crate::watch_cmd::{home, local, tilde};

/// A cleanup is a drop of at least this much in one period.
const CLEANUP: u64 = 1_000_000_000;

fn open(db: &Path) -> Result<Option<Store>> {
    if !db.exists() {
        println!(
            "No history yet. Start the watcher with `mdif agent install` (or run `mdif watch`)."
        );
        return Ok(None);
    }
    Ok(Some(Store::open(db)?))
}

pub fn owners(db: &Path, hours: i64, top: usize, json_out: bool) -> Result<()> {
    let Some(store) = open(db)? else {
        return Ok(());
    };
    let now = unix_now();
    let growth = store.growth_since(now - hours * HOUR)?;
    let regrowth = owners::regrowth(&store.growth_series(now - 30 * DAY)?, CLEANUP);
    if json_out {
        let out = json!({
            "hours": hours,
            "growth": growth.iter().map(|(o, b)| json!({
                "owner": o,
                "bytes": b,
                "per_day": *b as f64 * DAY as f64 / (hours * HOUR) as f64,
            })).collect::<Vec<_>>(),
            "cleanups": regrowth.iter().map(|r| json!({
                "owner": r.owner, "at": r.at, "freed": r.freed, "back": r.back,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if growth.is_empty() && regrowth.is_empty() {
        println!(
            "No folder growth recorded yet: the watcher adds it every 30 minutes, from the \
             second period on."
        );
        return Ok(());
    }
    let per_day = |b: i64| b as f64 * DAY as f64 / (hours * HOUR) as f64;
    let grew: Vec<_> = growth.iter().filter(|(_, b)| *b > 0).take(top).collect();
    if !grew.is_empty() {
        println!(
            "Grew the most in the last {}:",
            if hours == 1 {
                "hour".to_string()
            } else {
                format!("{hours} hours")
            }
        );
        for (o, b) in grew {
            println!(
                "  {:>9}  {:>9}/day  {}",
                bytes(*b as u64),
                bytes(per_day(*b) as u64),
                printable(&tilde(o))
            );
        }
    }
    let mut shrank: Vec<_> = growth.iter().filter(|(_, b)| *b < 0).collect();
    shrank.sort_by_key(|(_, b)| *b);
    if !shrank.is_empty() {
        println!();
        println!("Shrank:");
        for (o, b) in shrank.into_iter().take(top) {
            println!(
                "  {:>9}  {}",
                format!("-{}", bytes(b.unsigned_abs())),
                printable(&tilde(o))
            );
        }
    }
    if !regrowth.is_empty() {
        println!();
        println!("Cleanups in the last 30 days, and what came back:");
        for r in regrowth.iter().take(top) {
            let verdict = if r.share() >= 0.5 {
                ": it grows back, so clearing it by hand doesn't last"
            } else {
                ""
            };
            println!(
                "  {}: cleared {} on {}, {} back since ({:.0}%){verdict}",
                printable(&tilde(&r.owner)),
                bytes(r.freed),
                local(r.at).format("%b %-d %H:%M"),
                bytes(r.back.max(0) as u64),
                r.share() * 100.0
            );
        }
    }
    Ok(())
}

pub fn incidents(db: &Path, days: i64, json_out: bool) -> Result<()> {
    let Some(store) = open(db)? else {
        return Ok(());
    };
    let home = home();
    let episodes = store.incidents_since(unix_now() - days * DAY)?;
    if json_out {
        let out: Vec<_> = episodes
            .iter()
            .map(|e| {
                json!({
                    "opened": e.opened,
                    "updated": e.updated,
                    "closed": e.closed,
                    "incident": e.incident,
                    "sentence": e.incident.sentence(Some(&home)),
                    "fix": suggest::for_incident(&e.incident, &home),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if episodes.is_empty() {
        println!("No runaway disk fillers in the last {days} days.");
        return Ok(());
    }
    for e in &episodes {
        let when = match e.closed {
            None => format!(
                "since {} (still going)",
                local(e.opened).format("%b %-d %H:%M")
            ),
            Some(c) => format!(
                "{} – {}",
                local(e.opened).format("%b %-d %H:%M"),
                local(c).format("%b %-d %H:%M")
            ),
        };
        println!("[{}] {when}", e.incident.kind.as_str());
        println!("  {}", e.incident.sentence(Some(&home)));
        let fix = suggest::for_incident(&e.incident, &home);
        println!("  Fix: {}", tweaks_cmd::fix_line(&fix));
    }
    Ok(())
}
