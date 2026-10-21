//! What raises an alert, as the user set it (`mdif alerts`, and the app's
//! Settings → Notifications). Kept in the history's `meta` table under
//! `alerts.*`. Anything not set is the default, the plan's, so nothing
//! changes until the user sets something.
//!
//! Only thresholds can be set: how little free space is an alert, how far
//! ahead the forecast warns, how much a folder must grow and a process must
//! write. The clauses that keep false positives out stay as they are: a
//! steady fit, still growing, a share of the folder's own size; writing
//! steadily what the disk doesn't keep; a forecast alert that holds with any
//! day of the history left out. Whether each kind notifies is the app's to
//! decide from the `notify_*` flags; what is detected is shown either way.

use anyhow::{Result, anyhow};
use serde::Serialize;

use crate::forecast::{self, Alert, Forecast};
use crate::incidents::{MIN_GROWTH, MIN_WRITTEN_PER_HOUR};
use crate::store::Store;
use crate::units::{bytes, parse_bytes};

const PREFIX: &str = "alerts.";

const MB: u64 = 1_000_000;
const GB: u64 = 1_000_000_000;

/// The ranges values must be in, inclusive.
pub const LOW_FREE: (u64, u64) = (GB, 200 * GB);
pub const GROWTH_MIN: (u64, u64) = (100 * MB, 100 * GB);
pub const CHURN_PER_HOUR: (u64, u64) = (100 * MB, 50 * GB);
/// The forecast's levels: within a day, 3 days, 14 days.
pub const WITHIN_DAYS: [u32; 3] = [1, 3, 14];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    LowFree,
    FullWithinDays,
    GrowthMin,
    ChurnPerHour,
    NotifyForecast,
    NotifyGrowth,
    NotifyChurn,
}

impl Key {
    pub const ALL: [Key; 7] = [
        Key::LowFree,
        Key::FullWithinDays,
        Key::GrowthMin,
        Key::ChurnPerHour,
        Key::NotifyForecast,
        Key::NotifyGrowth,
        Key::NotifyChurn,
    ];

    /// As in the JSON and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Key::LowFree => "low_free",
            Key::FullWithinDays => "full_within_days",
            Key::GrowthMin => "growth_min",
            Key::ChurnPerHour => "churn_per_hour",
            Key::NotifyForecast => "notify_forecast",
            Key::NotifyGrowth => "notify_growth",
            Key::NotifyChurn => "notify_churn",
        }
    }

    /// `low_free`, `low-free` or `alerts.low_free`.
    pub fn parse(s: &str) -> Option<Key> {
        let s = s.trim();
        let s = s.strip_prefix(PREFIX).unwrap_or(s).replace('-', "_");
        Key::ALL.into_iter().find(|k| k.name() == s)
    }

    /// One line about what it does.
    pub fn about(self) -> &'static str {
        match self {
            Key::LowFree => "alert when less than this is free (`auto`: 10 GB or 2% of the disk)",
            Key::FullWithinDays => {
                "warn when the disk is likely full within this many days (1, 3 or 14)"
            }
            Key::GrowthMin => "a folder that keeps growing must grow this much to be a runaway",
            Key::ChurnPerHour => "a process must write this much in an hour to be wearing the disk",
            Key::NotifyForecast => "notify when the disk is filling",
            Key::NotifyGrowth => "notify about folders that keep growing",
            Key::NotifyChurn => "notify about processes wearing the disk",
        }
    }

    fn meta_key(self) -> String {
        format!("{PREFIX}{}", self.name())
    }
}

/// What the user set; `None`: the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Settings {
    pub low_free: Option<u64>,
    pub full_within_days: Option<u32>,
    pub growth_min: Option<u64>,
    pub churn_per_hour: Option<u64>,
    pub notify_forecast: Option<bool>,
    pub notify_growth: Option<bool>,
    pub notify_churn: Option<bool>,
}

/// The settings in effect on a disk of a given size: what `mdif alerts
/// --json` prints, and `mdif status --json` under `alerts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct InEffect {
    /// Free space below this is an alert; `None`: automatic.
    pub low_free: Option<u64>,
    /// The automatic floor on this disk: the larger of 10 GB and 2% of it.
    pub low_free_auto: u64,
    /// Forecast levels further out than this are not raised.
    pub full_within_days: u32,
    pub growth_min: u64,
    pub churn_per_hour: u64,
    pub notify_forecast: bool,
    pub notify_growth: bool,
    pub notify_churn: bool,
}

impl Settings {
    /// What is stored. A value that doesn't read or is out of range (edited
    /// by hand, or from another version) counts as not set.
    pub fn load(store: &Store) -> Result<Settings> {
        let mut s = Settings::default();
        for (k, v) in store.meta_prefixed(PREFIX)? {
            if let Some(key) = Key::parse(&k) {
                let _ = s.put(key, &v);
            }
        }
        Ok(s)
    }

    /// Check `value` for `key` and store it; `default` (or `auto`, for
    /// `low_free`) clears it, as does a value equal to the default. Returns
    /// the settings as they are now.
    pub fn set(store: &Store, key: Key, value: &str) -> Result<Settings> {
        let mut s = Settings::load(store)?;
        s.put(key, value).map_err(|e| anyhow!(e))?;
        match s.stored(key) {
            Some(v) => store.set_meta(&key.meta_key(), &v)?,
            None => store.delete_meta(&key.meta_key())?,
        }
        Ok(s)
    }

    /// Back to the defaults: `keys`, or all of them.
    pub fn reset(store: &Store, keys: &[Key]) -> Result<()> {
        let keys = if keys.is_empty() { &Key::ALL[..] } else { keys };
        for k in keys {
            store.delete_meta(&k.meta_key())?;
        }
        Ok(())
    }

    /// Whether `key` is set (not its default).
    pub fn is_set(&self, key: Key) -> bool {
        self.stored(key).is_some()
    }

    /// Free space below this is an alert.
    pub fn floor(&self, total: u64) -> u64 {
        self.low_free.unwrap_or_else(|| forecast::low_floor(total))
    }

    pub fn within_days(&self) -> u32 {
        self.full_within_days.unwrap_or(14)
    }

    /// The alert for `avail` bytes free of `total` and the forecast, with
    /// these thresholds.
    pub fn alert(&self, f: Option<&Forecast>, avail: u64, total: u64) -> Alert {
        forecast::alert_with(f, avail, self.floor(total), self.within_days())
    }

    pub fn in_effect(&self, total: u64) -> InEffect {
        InEffect {
            low_free: self.low_free,
            low_free_auto: forecast::low_floor(total),
            full_within_days: self.within_days(),
            growth_min: self.growth_min.unwrap_or(MIN_GROWTH),
            churn_per_hour: self.churn_per_hour.unwrap_or(MIN_WRITTEN_PER_HOUR),
            notify_forecast: self.notify_forecast.unwrap_or(true),
            notify_growth: self.notify_growth.unwrap_or(true),
            notify_churn: self.notify_churn.unwrap_or(true),
        }
    }

    fn put(&mut self, key: Key, value: &str) -> Result<(), String> {
        let v = value.trim();
        let clear = v.eq_ignore_ascii_case("default") || (key == Key::LowFree && v == "auto");
        let in_range = |(lo, hi): (u64, u64), what: &str| -> Result<Option<u64>, String> {
            match parse_bytes(v) {
                Some(b) if (lo..=hi).contains(&b) => Ok(Some(b)),
                _ => Err(format!(
                    "{} is {what}: between {} and {} (like `5GB` or `500MB`){}",
                    key.name(),
                    bytes(lo),
                    bytes(hi),
                    if key == Key::LowFree {
                        ", or `auto`"
                    } else {
                        ""
                    }
                )),
            }
        };
        let flag = || -> Result<Option<bool>, String> {
            match v.to_ascii_lowercase().as_str() {
                "true" | "on" | "yes" | "1" => Ok(Some(true)),
                "false" | "off" | "no" | "0" => Ok(Some(false)),
                _ => Err(format!("{} is true or false", key.name())),
            }
        };
        match key {
            _ if clear => self.clear(key),
            Key::LowFree => self.low_free = in_range(LOW_FREE, "free space")?,
            Key::FullWithinDays => {
                self.full_within_days = match v.parse::<u32>() {
                    Ok(d) if WITHIN_DAYS.contains(&d) => Some(d).filter(|&d| d != 14),
                    _ => return Err(format!("{} is 1, 3 or 14", key.name())),
                }
            }
            Key::GrowthMin => {
                self.growth_min = in_range(GROWTH_MIN, "bytes grown")?.filter(|&b| b != MIN_GROWTH)
            }
            Key::ChurnPerHour => {
                self.churn_per_hour = in_range(CHURN_PER_HOUR, "bytes written in an hour")?
                    .filter(|&b| b != MIN_WRITTEN_PER_HOUR)
            }
            Key::NotifyForecast => self.notify_forecast = flag()?.filter(|&b| !b),
            Key::NotifyGrowth => self.notify_growth = flag()?.filter(|&b| !b),
            Key::NotifyChurn => self.notify_churn = flag()?.filter(|&b| !b),
        }
        Ok(())
    }

    fn clear(&mut self, key: Key) {
        match key {
            Key::LowFree => self.low_free = None,
            Key::FullWithinDays => self.full_within_days = None,
            Key::GrowthMin => self.growth_min = None,
            Key::ChurnPerHour => self.churn_per_hour = None,
            Key::NotifyForecast => self.notify_forecast = None,
            Key::NotifyGrowth => self.notify_growth = None,
            Key::NotifyChurn => self.notify_churn = None,
        }
    }

    /// What `key` is stored as; `None`: not stored (the default).
    fn stored(&self, key: Key) -> Option<String> {
        match key {
            Key::LowFree => self.low_free.map(|b| b.to_string()),
            Key::FullWithinDays => self.full_within_days.map(|d| d.to_string()),
            Key::GrowthMin => self.growth_min.map(|b| b.to_string()),
            Key::ChurnPerHour => self.churn_per_hour.map(|b| b.to_string()),
            Key::NotifyForecast => self.notify_forecast.map(|b| b.to_string()),
            Key::NotifyGrowth => self.notify_growth.map(|b| b.to_string()),
            Key::NotifyChurn => self.notify_churn.map(|b| b.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOTAL: u64 = 500 * GB;

    #[test]
    fn nothing_set_is_todays_behaviour() {
        let store = Store::open_in_memory().unwrap();
        let s = Settings::load(&store).unwrap();
        assert_eq!(s, Settings::default());
        let e = s.in_effect(TOTAL);
        assert_eq!(
            e,
            InEffect {
                low_free: None,
                low_free_auto: 10 * GB,
                full_within_days: 14,
                growth_min: GB,
                churn_per_hour: GB,
                notify_forecast: true,
                notify_growth: true,
                notify_churn: true,
            }
        );
        assert_eq!(s.floor(TOTAL), forecast::low_floor(TOTAL));
        assert_eq!(s.floor(4_000 * GB), 80 * GB);
    }

    #[test]
    fn values_are_checked_stored_and_cleared() {
        let store = Store::open_in_memory().unwrap();
        let set = |k: &str, v: &str| Settings::set(&store, Key::parse(k).unwrap(), v);
        assert_eq!(set("low_free", "25GB").unwrap().low_free, Some(25 * GB));
        assert_eq!(
            set("full-within-days", "3").unwrap().full_within_days,
            Some(3)
        );
        assert_eq!(
            set("alerts.growth_min", "500 MB").unwrap().growth_min,
            Some(500 * MB)
        );
        assert_eq!(
            set("churn_per_hour", "2000000000").unwrap().churn_per_hour,
            Some(2 * GB)
        );
        assert_eq!(
            set("notify_churn", "off").unwrap().notify_churn,
            Some(false)
        );
        let s = Settings::load(&store).unwrap();
        let e = s.in_effect(TOTAL);
        assert_eq!(
            (
                e.low_free,
                e.full_within_days,
                e.growth_min,
                e.churn_per_hour
            ),
            (Some(25 * GB), 3, 500 * MB, 2 * GB)
        );
        assert!(!e.notify_churn && e.notify_growth && e.notify_forecast);
        assert_eq!(s.floor(TOTAL), 25 * GB);

        // Out of range, or not a value: refused, and nothing changes.
        for (k, v) in [
            ("low_free", "500MB"),
            ("low_free", "300GB"),
            ("full_within_days", "7"),
            ("growth_min", "50MB"),
            ("growth_min", "101GB"),
            ("churn_per_hour", "60GB"),
            ("churn_per_hour", "lots"),
            ("notify_growth", "maybe"),
        ] {
            assert!(set(k, v).is_err(), "{k} {v}");
        }
        assert_eq!(Settings::load(&store).unwrap(), s);
        assert!(Key::parse("nope").is_none());

        // Back to the defaults, by value or by name.
        assert_eq!(set("low_free", "auto").unwrap().low_free, None);
        assert_eq!(set("growth_min", "1GB").unwrap().growth_min, None);
        assert_eq!(set("notify_churn", "true").unwrap().notify_churn, None);
        assert_eq!(
            set("full_within_days", "default").unwrap().full_within_days,
            None
        );
        Settings::reset(&store, &[Key::ChurnPerHour]).unwrap();
        assert_eq!(Settings::load(&store).unwrap(), Settings::default());
        set("low_free", "20GB").unwrap();
        Settings::reset(&store, &[]).unwrap();
        assert_eq!(Settings::load(&store).unwrap(), Settings::default());
    }

    #[test]
    fn a_value_out_of_range_in_the_store_counts_as_not_set() {
        let store = Store::open_in_memory().unwrap();
        store.set_meta("alerts.low_free", "5").unwrap();
        store.set_meta("alerts.growth_min", "garbage").unwrap();
        store.set_meta("alerts.unknown", "1").unwrap();
        store.set_meta("alerts.notify_growth", "false").unwrap();
        let s = Settings::load(&store).unwrap();
        assert_eq!(
            s,
            Settings {
                notify_growth: Some(false),
                ..Settings::default()
            }
        );
    }

    #[test]
    fn the_floor_and_the_horizon_set_the_alert() {
        let mut s = Settings::default();
        assert_eq!(s.alert(None, 12 * GB, TOTAL), Alert::None);
        s.low_free = Some(15 * GB);
        assert_eq!(s.alert(None, 12 * GB, TOTAL), Alert::Low);
        // The forecast's levels, with a robust chance of 60% within 14 days
        // and 20% within 3.
        let f = Forecast {
            robust: Some(forecast::FullBy {
                d1: 0.0,
                d3: 0.2,
                d14: 0.6,
            }),
            ..forecast::forecast(
                &[crate::store::SpacePoint {
                    ts: 0,
                    total: TOTAL,
                    avail: 100 * GB,
                }],
                &forecast::Params::default(),
            )
            .unwrap()
        };
        let s = Settings::default();
        assert_eq!(s.alert(Some(&f), 100 * GB, TOTAL), Alert::Within14Days);
        let s3 = Settings {
            full_within_days: Some(3),
            ..s
        };
        assert_eq!(s3.alert(Some(&f), 100 * GB, TOTAL), Alert::None);
        let sooner = Forecast {
            robust: Some(forecast::FullBy {
                d1: 0.1,
                d3: 0.7,
                d14: 1.0,
            }),
            ..f.clone()
        };
        assert_eq!(s3.alert(Some(&sooner), 100 * GB, TOTAL), Alert::Within3Days);
        let s1 = Settings {
            full_within_days: Some(1),
            ..s
        };
        assert_eq!(s1.alert(Some(&sooner), 100 * GB, TOTAL), Alert::None);
    }
}
