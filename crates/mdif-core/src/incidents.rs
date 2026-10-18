//! Incidents: runaway disk fillers, said in one sentence, and only when
//! every clause holds (false positives are the whole product).
//!
//! **Growth**: an owner folder keeps getting bigger. All of these must hold
//! over the sustain window:
//! - its cumulative growth fits a rising line (r² ≥ 0.85, at least 8 points),
//!   which rules out bursts, staircases and sawtooth caches;
//! - it grew by at least 1 GB, and by at least 5% of its own size;
//! - it is still growing (the last third of the window grew at least a
//!   quarter of what the fitted rate predicts) and at its peak (within 2%),
//!   so whatever has plateaued or started shrinking is left alone;
//! - at the fitted rate it would fill the remaining free space within the
//!   horizon (7 days);
//! - no component of its path is on the allowlist.
//!
//! **Churn**: a process writes far more than the disk keeps, like a stuck
//! sync client rewriting logs. Over the last hour it wrote at least 1 GB,
//! steadily (in at least 3 of its 4 quarters), while the volume kept less
//! than 10% of that, and its name is not on the allowlist.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::forecast::{DAY, HOUR};
use crate::store::SpacePoint;
use crate::units::bytes;

#[derive(Debug, Clone)]
pub struct GrowthParams {
    pub sustain: i64,
    /// Cumulative growth is evaluated on this grid, so an owner that only
    /// reports every few periods still has evenly spaced points.
    pub grid: i64,
    pub min_points: usize,
    pub min_r2: f64,
    pub min_growth: u64,
    pub min_share: f64,
    pub peak_tolerance: f64,
    pub horizon: i64,
    /// Path components (exact) whose folders may grow as they like.
    pub allow: Vec<String>,
}

impl Default for GrowthParams {
    fn default() -> Self {
        GrowthParams {
            sustain: 6 * HOUR,
            grid: 1800,
            min_points: 8,
            min_r2: 0.85,
            min_growth: 1_000_000_000,
            min_share: 0.05,
            peak_tolerance: 0.02,
            horizon: 7 * DAY,
            allow: [
                // Deliberate: photo imports and virtual machines.
                "Photos Library.photoslibrary",
                "Virtual Machines.localized",
                "Parallels",
                "com.utmapp.UTM",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChurnParams {
    pub window: i64,
    pub min_written: u64,
    /// Quarters of the window that must each see at least an eighth of
    /// `min_written`.
    pub min_quarters: usize,
    pub max_kept: f64,
    /// Process names (exact) allowed to write a lot.
    pub allow: Vec<String>,
}

impl Default for ChurnParams {
    fn default() -> Self {
        ChurnParams {
            window: HOUR,
            min_written: 1_000_000_000,
            min_quarters: 3,
            max_kept: 0.10,
            allow: [
                // Backups, indexing and updates write a lot by design.
                "backupd",
                "mds",
                "mds_stores",
                "mdworker_shared",
                "softwareupdated",
                // Re-writes what its clients write into a disk image; the
                // clients are measured themselves.
                "diskimages-helper",
                // Builds.
                "rustc",
                "cargo",
                "clang",
                "swift-frontend",
                "ld",
                "ld64",
                "cc1",
                "cc1plus",
                "go",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Growth,
    Churn,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Growth => "growth",
            Kind::Churn => "churn",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Incident {
    pub kind: Kind,
    /// The owner folder (growth) or the process name (churn).
    pub subject: String,
    pub at: i64,
    pub window: i64,
    /// Grown (growth) or written (churn) over the window.
    pub amount: u64,
    /// The same as a rate.
    pub per_day: f64,
    /// Growth: r² of the fit. Churn: share of the writes the disk kept.
    pub fit: f64,
    /// Growth: the folder's size now, when it could be measured.
    pub size: Option<u64>,
    /// Growth: days until the disk is full at this rate.
    pub full_in_days: Option<f64>,
}

impl Incident {
    /// One sentence with the evidence. `home` is shown as `~`.
    pub fn sentence(&self, home: Option<&Path>) -> String {
        match self.kind {
            Kind::Growth => {
                let size = self
                    .size
                    .map(|s| format!(", now {}", bytes(s)))
                    .unwrap_or_default();
                let full = self
                    .full_in_days
                    .map(|d| format!(" At that rate the disk is full in {}.", days(d)))
                    .unwrap_or_default();
                format!(
                    "{} grew {} in {} ({} a day, steadily{size}).{full}",
                    display(&self.subject, home),
                    bytes(self.amount),
                    span(self.window),
                    bytes(self.per_day as u64),
                )
            }
            Kind::Churn => format!(
                "{} wrote {} in {} while the disk kept {}: it is rewriting the same data \
                 ({} a day of SSD wear). A sync or log that is stuck is the usual cause; \
                 restarting it usually stops it.",
                self.subject,
                bytes(self.amount),
                span(self.window),
                if self.fit < 0.005 {
                    "almost none of it".to_string()
                } else {
                    format!("{:.0}%", self.fit * 100.0)
                },
                bytes(self.per_day as u64),
            ),
        }
    }
}

/// Growth incidents from per-owner growth rows (`owner -> [(ts, bytes)]`).
/// `size_of` measures an owner, and is only called for owners that pass
/// every other clause.
pub fn growth(
    series: &HashMap<String, Vec<(i64, i64)>>,
    now: i64,
    avail: u64,
    mut size_of: impl FnMut(&str) -> Option<u64>,
    p: &GrowthParams,
) -> Vec<Incident> {
    let since = now - p.sustain;
    let mut out = Vec::new();
    let mut owners: Vec<&String> = series.keys().collect();
    owners.sort();
    for owner in owners {
        if Path::new(owner)
            .iter()
            .any(|c| p.allow.iter().any(|a| c == a.as_str()))
        {
            continue;
        }
        let rows: Vec<(i64, i64)> = series[owner]
            .iter()
            .copied()
            .filter(|(ts, _)| *ts > since && *ts <= now)
            .collect();
        if rows.is_empty() {
            continue;
        }
        // Cumulative growth since the start of the window, on the grid.
        let mut t = Vec::new();
        let mut y = Vec::new();
        let mut g = since;
        while g <= now {
            t.push((g - since) as f64);
            y.push(
                rows.iter()
                    .filter(|(ts, _)| *ts <= g)
                    .map(|(_, b)| *b)
                    .sum::<i64>() as f64,
            );
            g += p.grid;
        }
        if t.len() < p.min_points {
            continue;
        }
        let Some((slope, r2)) = fit(&t, &y) else {
            continue;
        };
        let grown = rows.iter().map(|(_, b)| b).sum::<i64>();
        if slope <= 0.0 || r2 < p.min_r2 || grown < p.min_growth as i64 {
            continue;
        }
        let peak = y.iter().copied().fold(f64::MIN, f64::max);
        let last = *y.last().expect("at least min_points");
        // A third of the window is also the longest a big folder waits to
        // be re-listed, so its staircase still shows growth here.
        let third = y.len() - 1 - (y.len() - 1) / 3;
        let recent = last - y[third];
        if recent < 0.25 * slope * (t[t.len() - 1] - t[third]) {
            continue;
        }
        let full_in = avail as f64 / slope / DAY as f64;
        if full_in * DAY as f64 > p.horizon as f64 {
            continue;
        }
        let size = size_of(owner);
        let reference = size.map_or(grown as f64, |s| s as f64);
        if peak - last > p.peak_tolerance * reference {
            continue;
        }
        if let Some(s) = size
            && (grown as f64) < p.min_share * s as f64
        {
            continue;
        }
        out.push(Incident {
            kind: Kind::Growth,
            subject: owner.clone(),
            at: now,
            window: p.sustain,
            amount: grown as u64,
            per_day: slope * DAY as f64,
            fit: r2,
            size,
            full_in_days: Some(full_in),
        });
    }
    out
}

/// Churn incidents from per-process writes (`name -> [(ts, bytes)]`) and
/// the volume's free space over the same time.
pub fn churn(
    writes: &HashMap<String, Vec<(i64, u64)>>,
    space: &[SpacePoint],
    now: i64,
    p: &ChurnParams,
) -> Vec<Incident> {
    let since = now - p.window;
    let in_window: Vec<&SpacePoint> = space
        .iter()
        .filter(|s| s.ts >= since && s.ts <= now)
        .collect();
    let consumed = match (in_window.first(), in_window.last()) {
        (Some(a), Some(b)) => (a.avail as i64 - b.avail as i64).max(0) as u64,
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut names: Vec<&String> = writes.keys().collect();
    names.sort();
    for name in names {
        if name.starts_with('(') || p.allow.iter().any(|a| a == name) {
            continue;
        }
        let rows: Vec<(i64, u64)> = writes[name]
            .iter()
            .copied()
            .filter(|(ts, _)| *ts > since && *ts <= now)
            .collect();
        let written: u64 = rows.iter().map(|(_, b)| b).sum();
        if written < p.min_written {
            continue;
        }
        let quarter = p.window / 4;
        let steady = (0..4)
            .filter(|q| {
                let (a, b) = (since + q * quarter, since + (q + 1) * quarter);
                let w: u64 = rows
                    .iter()
                    .filter(|(ts, _)| *ts > a && *ts <= b)
                    .map(|(_, b)| b)
                    .sum();
                w >= p.min_written / 8
            })
            .count();
        let kept = consumed as f64 / written as f64;
        if steady < p.min_quarters || kept > p.max_kept {
            continue;
        }
        out.push(Incident {
            kind: Kind::Churn,
            subject: name.clone(),
            at: now,
            window: p.window,
            amount: written,
            per_day: written as f64 * DAY as f64 / p.window as f64,
            fit: kept,
            size: None,
            full_in_days: None,
        });
    }
    out
}

/// Least-squares slope and r².
fn fit(t: &[f64], y: &[f64]) -> Option<(f64, f64)> {
    let n = t.len() as f64;
    let (mt, my) = (t.iter().sum::<f64>() / n, y.iter().sum::<f64>() / n);
    let sxx: f64 = t.iter().map(|x| (x - mt).powi(2)).sum();
    let syy: f64 = y.iter().map(|v| (v - my).powi(2)).sum();
    let sxy: f64 = t.iter().zip(y).map(|(x, v)| (x - mt) * (v - my)).sum();
    if sxx == 0.0 || syy == 0.0 {
        return None;
    }
    Some((sxy / sxx, sxy * sxy / (sxx * syy)))
}

fn display(path: &str, home: Option<&Path>) -> String {
    let text = match home.and_then(|h| Path::new(path).strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.to_string(),
    };
    crate::text::printable(&text).into_owned()
}

fn span(secs: i64) -> String {
    if secs >= 2 * HOUR {
        format!("{} hours", secs / HOUR)
    } else if secs >= 2 * 60 {
        format!("{} minutes", secs / 60)
    } else {
        format!("{secs} seconds")
    }
}

fn days(d: f64) -> String {
    let n = if d < 1.0 {
        (d * 24.0).round().max(1.0)
    } else {
        d.round()
    };
    let unit = if d < 1.0 { "hour" } else { "day" };
    if n == 1.0 {
        format!("1 {unit}")
    } else {
        format!("{n} {unit}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: i64 = 1_000_000_000;
    const NOW: i64 = 100 * DAY;

    /// A row every 30 minutes over `hours`, each `per_row(i)`.
    fn rows(hours: i64, per_row: impl Fn(i64) -> i64) -> Vec<(i64, i64)> {
        (1..=hours * 2)
            .map(|i| (NOW - hours * HOUR + i * 1800, per_row(i)))
            .collect()
    }

    fn one(owner: &str, r: Vec<(i64, i64)>) -> HashMap<String, Vec<(i64, i64)>> {
        HashMap::from([(owner.to_string(), r)])
    }

    fn grows(owner: &str, r: Vec<(i64, i64)>, avail: u64, size: Option<u64>) -> Vec<Incident> {
        growth(
            &one(owner, r),
            NOW,
            avail,
            |_| size,
            &GrowthParams::default(),
        )
    }

    #[test]
    fn steady_growth_that_would_fill_the_disk_is_an_incident() {
        // 400 MB every half hour = 19 GB/day, 40 GB free: full in about 2 days.
        let inc = grows(
            "/Users/me/Library/Logs/x",
            rows(6, |_| GB * 4 / 10),
            40 * GB as u64,
            Some(10 * GB as u64),
        );
        assert_eq!(inc.len(), 1);
        let i = &inc[0];
        assert_eq!(i.kind, Kind::Growth);
        assert!((i.per_day / GB as f64 - 19.2).abs() < 0.5, "{i:?}");
        assert!(i.fit > 0.99);
        assert!((i.full_in_days.unwrap() - 2.1).abs() < 0.2);
        let s = i.sentence(Some(Path::new("/Users/me")));
        assert!(
            s.starts_with("~/Library/Logs/x grew 4.8 GB in 6 hours"),
            "{s}"
        );
        assert!(s.contains("full in 2 days"), "{s}");
    }

    #[test]
    fn a_big_folder_listed_every_two_hours_still_counts() {
        // Deferred listings: 1.6 GB every fourth period instead of 400 MB each.
        let inc = grows(
            "/a",
            rows(6, |i| if i % 4 == 0 { 16 * GB / 10 } else { 0 }),
            40 * GB as u64,
            None,
        );
        assert_eq!(inc.len(), 1, "staircase of a deferred folder");
    }

    #[test]
    fn growth_clauses_reject_what_is_not_a_runaway() {
        let free = 40 * GB as u64;
        // A burst: everything in one period.
        assert!(
            grows(
                "/a",
                rows(6, |i| if i == 6 { 5 * GB } else { 0 }),
                free,
                None
            )
            .is_empty()
        );
        // Plateaued: grew, then stopped for the last two hours.
        assert!(grows("/a", rows(6, |i| if i <= 8 { GB } else { 0 }), free, None).is_empty());
        // Sawtooth: grows and is cleared.
        assert!(
            grows(
                "/a",
                rows(6, |i| if i % 4 == 0 { -3 * GB / 2 } else { GB / 2 }),
                free,
                None
            )
            .is_empty()
        );
        // Too small: 50 MB in 6 hours.
        assert!(grows("/a", rows(6, |_| 4_000_000), free, None).is_empty());
        // Slow on a roomy disk: full in years.
        assert!(grows("/a", rows(6, |_| GB / 5), 5_000 * GB as u64, None).is_empty());
        // Small next to its own size: 2.4 GB on a 400 GB VM folder.
        assert!(grows("/a", rows(6, |_| GB / 5), free, Some(400 * GB as u64)).is_empty());
        // Allowlisted by a whole path component, never a substring.
        let photos = "/Users/me/Pictures/Photos Library.photoslibrary";
        assert!(grows(photos, rows(6, |_| GB / 2), free, None).is_empty());
        let near = "/Users/me/Pictures/Photos Library.photoslibrary-old";
        assert_eq!(grows(near, rows(6, |_| GB / 2), free, None).len(), 1);
    }

    fn churn_case(per_5min: u64, consumed: u64, minutes: i64) -> Vec<Incident> {
        let writes = HashMap::from([(
            "OneDrive".to_string(),
            (1..=minutes / 5)
                .map(|i| (NOW - minutes * 60 + i * 300, per_5min))
                .collect(),
        )]);
        let space = [
            SpacePoint {
                ts: NOW - HOUR,
                total: 500,
                avail: 100 * GB as u64,
            },
            SpacePoint {
                ts: NOW,
                total: 500,
                avail: 100 * GB as u64 - consumed,
            },
        ];
        churn(&writes, &space, NOW, &ChurnParams::default())
    }

    #[test]
    fn steady_rewriting_that_the_disk_doesnt_keep_is_churn() {
        // 250 MB every 5 minutes = 3 GB/hour, disk kept 20 MB.
        let inc = churn_case(250_000_000, 20_000_000, 60);
        assert_eq!(inc.len(), 1);
        let s = inc[0].sentence(None);
        assert!(
            s.starts_with("OneDrive wrote 3.0 GB in 60 minutes while the disk kept 1%"),
            "{s}"
        );
    }

    #[test]
    fn churn_clauses_reject_growth_bursts_and_allowed_writers() {
        // Writes that the disk keeps are growth, not churn.
        assert!(churn_case(250_000_000, 2_500_000_000, 60).is_empty());
        // A 15-minute burst isn't sustained.
        assert!(churn_case(400_000_000, 0, 15).is_empty());
        // Below 1 GB/hour.
        assert!(churn_case(50_000_000, 0, 60).is_empty());
        // Builds are allowed to write.
        let writes = HashMap::from([(
            "rustc".to_string(),
            (1..=12)
                .map(|i| (NOW - HOUR + i * 300, 500_000_000))
                .collect(),
        )]);
        let space = [
            SpacePoint {
                ts: NOW - HOUR,
                total: 1,
                avail: 10,
            },
            SpacePoint {
                ts: NOW,
                total: 1,
                avail: 10,
            },
        ];
        assert!(churn(&writes, &space, NOW, &ChurnParams::default()).is_empty());
    }
}
