//! `mdif status`: free space, the forecast and what needs attention, in a few
//! lines. With `--json` it is what the menu bar app reads every few minutes,
//! so it only reads the history: nothing here measures a folder.

use std::path::Path;

use anyhow::{Context, Result};
use mdif_core::alerts;
use mdif_core::forecast::{self, Alert, DAY, Forecast, Params};
use mdif_core::privacy;
use mdif_core::store::{Episode, Store};
use mdif_core::tweaks::{self, Status, Tweak};
use mdif_core::units::bytes;
use mdif_core::watch::{self, unix_now};
use serde_json::json;

use crate::owners_cmd::episode_json;
use crate::tweaks_cmd::{fix_line, status_word};
use crate::watch_cmd::{self, basis, date, days, duration, full_range, home, percent};

/// A tweak MyDiskIsFull applied and hasn't reverted, as it reads now.
struct Applied {
    tweak: &'static Tweak,
    at: i64,
    /// `None`: its settings couldn't be read.
    status: Option<Status>,
}

impl Applied {
    /// Undone since, by an update or the app itself.
    fn undone(&self) -> bool {
        matches!(self.status, Some(Status::NotApplied | Status::Partial))
    }
}

pub fn status(db: &Path, volume: &Path, json_out: bool) -> Result<()> {
    let st =
        mdif_macos::fs::statfs(volume).with_context(|| format!("statfs {}", volume.display()))?;
    let now = unix_now();
    let home = home();
    // The app runs this every few minutes, unattended: a privacy prompt
    // would wait for an answer (see mdif_core::privacy).
    let probes = mdif_macos::sys::access_probes(&home);
    privacy::avoid_prompts(mdif_macos::sys::full_disk_access(&probes) != Some(true));
    let store = if db.exists() {
        Some(Store::open(db)?)
    } else {
        None
    };
    let pts = match &store {
        Some(s) => s.space_since(&st.mount_point, now - 400 * DAY)?,
        None => Vec::new(),
    };
    let cleanups = match (&store, pts.first()) {
        (Some(s), Some(first)) => s.cleanups_since(first.ts)?,
        _ => Vec::new(),
    };
    // The thresholds the user set (`mdif alerts`), and the defaults for the
    // rest.
    let user = match &store {
        Some(s) => alerts::Settings::load(s)?,
        None => alerts::Settings::default(),
    };
    let f = forecast::forecast_with(&pts, &cleanups, &Params::default());
    let alert = user.alert(f.as_ref(), st.avail, st.total);
    let keep = crate::keep_cmd::load_or_warn(db);
    let open: Vec<Episode> = match &store {
        Some(s) => s
            .incidents_since(now)?
            .into_iter()
            .filter(|e| e.closed.is_none())
            .collect(),
        None => Vec::new(),
    };
    let applied = match &store {
        Some(s) => applied(s)?,
        None => Vec::new(),
    };
    let access = match &store {
        Some(s) => watch::recorded_access(s)?,
        None => None,
    };
    let cost = match &store {
        Some(s) => watch::recorded_cost(s)?,
        None => None,
    };
    let interval = watch_cmd::sampling_interval(&pts);

    if json_out {
        let out = json!({
            "volume": st.mount_point,
            "total": st.total,
            "avail": st.avail,
            "floor": user.floor(st.total),
            "alert": alert,
            "alert_sentence": alert_sentence(alert, f.as_ref(), st.avail),
            "forecast": f,
            "history": {
                "samples": pts.len(),
                "first": pts.first().map(|p| p.ts),
                "last": pts.last().map(|p| p.ts),
                "interval_secs": interval,
                "bytes": watch_cmd::history_bytes(db),
            },
            "incidents": open.iter().map(|e| episode_json(e, &home, &keep)).collect::<Vec<_>>(),
            "tweaks": applied.iter().map(|a| json!({
                "id": a.tweak.id,
                "title": a.tweak.title,
                "status": a.status.map_or("unreadable", status_word),
                "applied": a.at,
                "undone": a.undone(),
            })).collect::<Vec<_>>(),
            "access": access.map(|(at, granted)| json!({
                "checked": at,
                "full_disk_access": granted,
            })),
            "watcher_cost": cost.map(|c| watch_cmd::cost_json(&c, interval)),
            "alerts": user.in_effect(st.total),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("{} free of {}.", bytes(st.avail), bytes(st.total));
    match (&store, &f) {
        (None, _) => {
            println!("No history yet: `mdif agent install` starts the watcher.");
            return Ok(());
        }
        (Some(_), None) => {
            println!("No samples yet. Is the watcher running? (`mdif agent status`)")
        }
        (Some(_), Some(f)) => println!("{}", trend_line(f, pts.len())),
    }
    if let Some(s) = alert_sentence(alert, f.as_ref(), st.avail) {
        println!("Alert: {s}");
    }
    for e in &open {
        println!("Runaway: {}", e.incident.sentence(Some(&home)));
        let fix = mdif_core::suggest::for_incident(&e.incident, &home, &keep);
        println!("  Fix: {}", fix_line(&fix));
    }
    for a in applied.iter().filter(|a| a.undone()) {
        println!(
            "Undone since: {}. `mdif tweaks apply {}` restores it.",
            a.tweak.title, a.tweak.id
        );
    }
    if let Some(last) = pts.last() {
        let age = now - last.ts;
        if age > 3 * interval.max(60) {
            println!(
                "Watcher: no sample for {}. Is it running? (`mdif agent status`)",
                duration(age)
            );
        }
    }
    if let Some((_, Some(false))) = access {
        println!(
            "Watcher: without Full Disk Access it can't see into other apps' folders, so growth \
             there goes unseen."
        );
    }
    Ok(())
}

/// Tweaks with a record not reverted, the most recent apply of each.
fn applied(store: &Store) -> Result<Vec<Applied>> {
    let mut out: Vec<Applied> = Vec::new();
    for r in store.all_tweaks()? {
        if r.reverted.is_some() || out.iter().any(|a| a.tweak.id == r.id) {
            continue;
        }
        let Some(tweak) = tweaks::find(&r.id) else {
            continue;
        };
        out.push(Applied {
            tweak,
            at: r.applied,
            status: tweak.record_status(&r).ok(),
        });
    }
    Ok(out)
}

fn trend_line(f: &Forecast, samples: usize) -> String {
    match (f.rate, f.full_in) {
        (None, _) => format!(
            "Collecting history ({samples} samples): a forecast needs at least 8 covering 3 hours."
        ),
        (Some(r), Some(full)) => format!(
            "Full in about {}, around {} ({}; about {} a day, {}).",
            days(full.days),
            date(f.at, full.days),
            full_range(f.at, &full),
            bytes(r.per_day.max(0.0) as u64),
            basis(f, &r)
        ),
        (Some(r), None) if r.per_day <= 0.0 => {
            format!("Not filling ({}).", basis(f, &r))
        }
        (Some(r), None) => format!(
            "Filling at about {} a day, but not full for a long time ({}).",
            bytes(r.per_day as u64),
            basis(f, &r)
        ),
    }
}

/// One sentence for the notification of `alert`.
fn alert_sentence(alert: Alert, f: Option<&Forecast>, avail: u64) -> Option<String> {
    if alert == Alert::Low {
        return Some(format!("Only {} is free on the disk.", bytes(avail)));
    }
    if alert == Alert::None {
        return None;
    }
    let f = f?;
    let (r, full, by) = (f.rate?, f.full_in?, f.full_by?);
    let (p, within) = match alert {
        Alert::WithinADay => (by.d1, "a day"),
        Alert::Within3Days => (by.d3, "3 days"),
        _ => (by.d14, "14 days"),
    };
    Some(format!(
        "At about {} a day, the disk is full in about {}, around {} ({} likely within {within}).",
        bytes(r.per_day.max(0.0) as u64),
        days(full.days),
        date(f.at, full.days),
        percent(p)
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use mdif_core::incidents::{Incident, Kind};
    use mdif_core::suggest::Fix;
    use serde_json::Value;

    use super::*;

    /// The fixture the app's contract check decodes (app/Tests): what
    /// `status --json` prints when something is open, which a fresh history
    /// on CI can't produce. Held here to the real serialization.
    const FIXTURE: &str = include_str!("../../../app/Tests/status-incidents.json");

    fn keys(v: &Value) -> BTreeSet<String> {
        v.as_object().unwrap().keys().cloned().collect()
    }

    #[test]
    fn the_apps_fixture_has_the_shape_status_prints() {
        let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
        let shown = &fixture["incidents"];
        let e = Episode {
            opened: 1,
            updated: 2,
            closed: None,
            incident: Incident {
                kind: Kind::Churn,
                subject: "SyncClient".into(),
                at: 2,
                window: 3600,
                amount: 5_000_000_000,
                per_day: 1.2e11,
                fit: 0.01,
                size: None,
                full_in_days: None,
            },
        };
        let printed = episode_json(&e, Path::new("/Users/me"), &Default::default());
        assert_eq!(keys(&printed), keys(&shown[0]));
        assert_eq!(keys(&printed["incident"]), keys(&shown[0]["incident"]));
        assert_eq!(printed["incident"]["kind"], "churn");

        // Every kind of fix, as serde writes it.
        let detail = |i: usize| shown[i]["fix"]["detail"].clone();
        let fixes = [
            Fix::Steps(detail(0).as_str().unwrap().into()),
            Fix::Run {
                command: detail(1)["command"].as_str().unwrap().into(),
                note: detail(1)["note"].as_str().unwrap().into(),
            },
            Fix::Tweak("gradle-caches"),
            Fix::Leave(detail(3).as_str().unwrap().into()),
        ];
        for (i, fix) in fixes.iter().enumerate() {
            assert_eq!(
                serde_json::to_value(fix).unwrap(),
                shown[i]["fix"],
                "fix {i}"
            );
        }
        assert_eq!(
            serde_json::to_value(Alert::Within3Days).unwrap(),
            fixture["alert"]
        );
        let tweak = &fixture["tweaks"][0];
        assert_eq!(
            keys(tweak),
            ["applied", "id", "status", "title", "undone"]
                .map(String::from)
                .into()
        );
        assert_eq!(
            tweak["status"],
            status_word(mdif_core::tweaks::Status::NotApplied)
        );
    }

    /// The fixture's forecast has the fields a simulated forecast prints,
    /// nested ones included.
    #[test]
    fn the_apps_fixture_has_the_forecast_status_prints() {
        use mdif_core::store::SpacePoint;
        let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
        let shown = &fixture["forecast"];
        // Three days filling 7.5 GB a day, with a cleanup on the second.
        let pts: Vec<SpacePoint> = (0..=72)
            .map(|k: i64| SpacePoint {
                ts: 1_792_146_600 + k * 3600,
                total: 494_384_795_648,
                avail: (43.5e9 - 7.5e9 * k as f64 / 24.0 + if k > 30 { 4e9 } else { 0.0 }) as u64,
            })
            .collect();
        let f = forecast::forecast(&pts, &Params::default()).unwrap();
        assert_eq!(f.method, forecast::Method::Simulation);
        let printed = serde_json::to_value(&f).unwrap();
        assert_eq!(keys(&printed), keys(shown));
        assert_eq!(printed["method"], shown["method"]);
        for k in ["rate", "trend", "full_in", "full_by", "paces"] {
            assert_eq!(keys(&printed[k]), keys(&shown[k]), "{k}");
        }
        assert_eq!(keys(&printed["steps"][0]), keys(&shown["steps"][0]));
        // The sentence the notification shows, as the fixture has it.
        let sentence = alert_sentence(Alert::Within3Days, Some(&f), f.avail).unwrap();
        let words = |s: &str| s.split(' ').take(4).collect::<Vec<_>>().join(" ");
        assert_eq!(
            words(&sentence),
            words(fixture["alert_sentence"].as_str().unwrap())
        );
    }
}
