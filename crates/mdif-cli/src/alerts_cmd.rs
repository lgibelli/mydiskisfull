//! `mdif alerts`: what raises an alert and a notification, as the user set
//! it (see mdif_core::alerts). Kept in the history; `mdif status --json`
//! prints the same under `alerts`.

use std::path::Path;

use anyhow::{Context, Result, bail};
use mdif_core::alerts::{InEffect, Key, Settings};
use mdif_core::store::Store;
use mdif_core::text::printable;
use mdif_core::units::bytes;

pub fn list(db: &Path, volume: &Path, json_out: bool) -> Result<()> {
    let total = total(volume)?;
    // Listing never makes a history: without one, all are the defaults.
    let s = if db.exists() {
        Settings::load(&Store::open(db)?)?
    } else {
        Settings::default()
    };
    let e = s.in_effect(total);
    if json_out {
        println!("{}", serde_json::to_string_pretty(&e)?);
        return Ok(());
    }
    println!(
        "What raises an alert (`mdif alerts set <key> <value>`; `mdif alerts reset` for the \
         defaults):"
    );
    for k in Key::ALL {
        let note = if s.is_set(k) { "" } else { " (default)" };
        println!(
            "  {:<17} {:<24} {}",
            k.name(),
            format!("{}{note}", value(&e, k)),
            k.about()
        );
    }
    Ok(())
}

pub fn set(db: &Path, volume: &Path, key: &str, value: &str, json_out: bool) -> Result<()> {
    let key = parse(key)?;
    let total = total(volume)?;
    let store = Store::open(db)?;
    let s = Settings::set(&store, key, value)?;
    if !json_out {
        println!("{}: {}.", key.name(), self::value(&s.in_effect(total), key));
        println!();
    }
    list(db, volume, json_out)
}

pub fn reset(db: &Path, volume: &Path, keys: &[String], json_out: bool) -> Result<()> {
    let keys: Vec<Key> = keys.iter().map(|k| parse(k)).collect::<Result<_>>()?;
    if db.exists() {
        Settings::reset(&Store::open(db)?, &keys)?;
    }
    if !json_out {
        println!(
            "Back to the default: {}.",
            if keys.is_empty() {
                "all of them".to_string()
            } else {
                keys.iter().map(|k| k.name()).collect::<Vec<_>>().join(", ")
            }
        );
        println!();
    }
    list(db, volume, json_out)
}

fn parse(key: &str) -> Result<Key> {
    match Key::parse(key) {
        Some(k) => Ok(k),
        None => {
            let keys: Vec<&str> = Key::ALL.iter().map(|k| k.name()).collect();
            bail!("no setting called {}: {}", printable(key), keys.join(", "))
        }
    }
}

/// The volume's size, which the automatic floor depends on.
fn total(volume: &Path) -> Result<u64> {
    Ok(mdif_macos::fs::statfs(volume)
        .with_context(|| format!("statfs {}", volume.display()))?
        .total)
}

fn value(e: &InEffect, k: Key) -> String {
    let on = |b: bool| if b { "on" } else { "off" }.to_string();
    match k {
        Key::LowFree => match e.low_free {
            Some(b) => bytes(b),
            None => format!("auto, {}", bytes(e.low_free_auto)),
        },
        Key::FullWithinDays if e.full_within_days == 1 => "1 day".into(),
        Key::FullWithinDays => format!("{} days", e.full_within_days),
        Key::GrowthMin => bytes(e.growth_min),
        Key::ChurnPerHour => bytes(e.churn_per_hour),
        Key::NotifyForecast => on(e.notify_forecast),
        Key::NotifyGrowth => on(e.notify_growth),
        Key::NotifyChurn => on(e.notify_churn),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::*;

    /// The fixtures the app's contract check decodes (app/Tests): settings
    /// that were changed, which a fresh history can't print, and the same
    /// object in `status --json`.
    const FIXTURE: &str = include_str!("../../../app/Tests/alerts.json");
    const STATUS: &str = include_str!("../../../app/Tests/status-incidents.json");

    fn keys(v: &Value) -> BTreeSet<String> {
        v.as_object().unwrap().keys().cloned().collect()
    }

    #[test]
    fn the_apps_alerts_fixture_has_the_shape_alerts_prints() {
        let store = Store::open_in_memory().unwrap();
        let s = Settings::set(&store, Key::LowFree, "20GB").unwrap();
        let printed = serde_json::to_value(s.in_effect(500_000_000_000)).unwrap();
        let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(keys(&printed), keys(&fixture));
        assert_eq!(printed["low_free"], fixture["low_free"]);
        assert_eq!(printed["low_free_auto"], fixture["low_free_auto"]);
        let status: Value = serde_json::from_str(STATUS).unwrap();
        let defaults =
            serde_json::to_value(Settings::default().in_effect(494_384_795_648)).unwrap();
        assert_eq!(status["alerts"], defaults);
        assert_eq!(status["floor"], defaults["low_free_auto"]);
    }
}
