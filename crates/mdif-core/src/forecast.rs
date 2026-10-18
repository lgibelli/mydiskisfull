//! When will the disk be full?
//!
//! The consumption rate is a Theil–Sen slope (median of all pairwise slopes)
//! of free space over several windows: robust to noise and to the odd outlier,
//! and without assumptions about the distribution.
//!
//! Space that comes back in one go (a cleanup, a deleted snapshot, an emptied
//! Trash) is a *step*, not a trend: one or two of those in a window are taken
//! out, so freeing 20 GB doesn't read as "the disk is shrinking" and the
//! forecast keeps reflecting how fast things fill. Steps that *recur* (three
//! or more in a window, e.g. a cache an app clears every few days) are the
//! pattern itself and stay in, so a sawtooth is forecast by its net rate
//! instead of raising a false alarm.

use serde::Serialize;

use crate::store::SpacePoint;

pub const HOUR: i64 = 3_600;
pub const DAY: i64 = 86_400;

/// Windows tried, longest first. The longest one with enough data is the
/// forecast; the shortest is reported as the recent rate.
pub const WINDOWS: [i64; 3] = [14 * DAY, 3 * DAY, 6 * HOUR];

#[derive(Debug, Clone)]
pub struct Params {
    /// Smallest jump in free space that counts as a step.
    pub step_min: u64,
    /// A step must also exceed this many times the typical change per sample.
    pub step_noise_factor: f64,
    /// Steps per window from which they count as a recurring pattern.
    pub recurring_steps: usize,
    pub min_points: usize,
    /// Share of a window the data must cover for that window to be used.
    pub min_coverage: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            step_min: 1_000_000_000,
            step_noise_factor: 10.0,
            recurring_steps: 3,
            min_points: 8,
            min_coverage: 0.5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Step {
    pub ts: i64,
    /// Bytes that came back.
    pub freed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Rate {
    pub window: i64,
    pub points: usize,
    /// Bytes consumed per day (negative: the disk is getting emptier).
    pub per_day: f64,
    /// 10th–90th percentile of the pairwise slopes, as bytes/day.
    pub low: f64,
    pub high: f64,
    /// Whether steps in this window were taken out (one-off cleanups) or
    /// kept (a recurring pattern).
    pub steps_removed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct FullIn {
    /// At the forecast rate.
    pub days: f64,
    /// At the fastest plausible rate (90th percentile).
    pub earliest: f64,
    /// At the slowest plausible rate; `None` if that rate isn't filling.
    pub latest: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Forecast {
    pub at: i64,
    pub avail: u64,
    /// The longest window with enough data.
    pub rate: Option<Rate>,
    /// The shortest window, when it differs from `rate`.
    pub recent: Option<Rate>,
    pub full_in: Option<FullIn>,
    /// When the recent rate would fill the disk much sooner (> 2× faster).
    pub recent_full_in: Option<FullIn>,
    /// Steps seen in the longest usable window, oldest first.
    pub steps: Vec<Step>,
}

pub fn forecast(points: &[SpacePoint], p: &Params) -> Option<Forecast> {
    let last = *points.last()?;
    let rates: Vec<(Rate, Vec<Step>)> = WINDOWS
        .iter()
        .filter_map(|&w| window_rate(points, last.ts - w, w, p))
        .collect();
    let (rate, steps) = match rates.first() {
        Some((r, s)) => (Some(*r), s.clone()),
        None => (None, Vec::new()),
    };
    let recent = rates
        .last()
        .map(|(r, _)| *r)
        .filter(|r| Some(r.window) != rate.map(|x| x.window));
    let full = rate.and_then(|r| full_in(last.avail, &r));
    let recent_full = match (recent, rate) {
        (Some(rc), Some(r)) if rc.per_day > 2.0 * r.per_day.max(0.0) => full_in(last.avail, &rc),
        _ => None,
    };
    Some(Forecast {
        at: last.ts,
        avail: last.avail,
        rate,
        recent,
        full_in: full,
        recent_full_in: recent_full,
        steps,
    })
}

fn full_in(avail: u64, r: &Rate) -> Option<FullIn> {
    if r.per_day <= 0.0 {
        return None;
    }
    let avail = avail as f64;
    Some(FullIn {
        days: avail / r.per_day,
        earliest: avail / r.high.max(r.per_day),
        latest: (r.low > 0.0).then(|| avail / r.low),
    })
}

fn window_rate(
    points: &[SpacePoint],
    since: i64,
    window: i64,
    p: &Params,
) -> Option<(Rate, Vec<Step>)> {
    let pts: Vec<&SpacePoint> = points.iter().filter(|pt| pt.ts >= since).collect();
    if pts.len() < p.min_points {
        return None;
    }
    let span = pts[pts.len() - 1].ts - pts[0].ts;
    if (span as f64) < p.min_coverage * window as f64 {
        return None;
    }

    // Changes between consecutive samples; positive = space came back.
    let deltas: Vec<f64> = pts
        .windows(2)
        .map(|w| w[1].avail as f64 - w[0].avail as f64)
        .collect();
    let mut abs: Vec<f64> = deltas.iter().map(|d| d.abs()).collect();
    let noise = median(&mut abs).unwrap_or(0.0);
    let threshold = (p.step_min as f64).max(p.step_noise_factor * noise);
    let step_idx: Vec<usize> = deltas
        .iter()
        .enumerate()
        .filter(|&(_, &d)| d >= threshold)
        .map(|(i, _)| i)
        .collect();
    let steps: Vec<Step> = step_idx
        .iter()
        .map(|&i| Step {
            ts: pts[i + 1].ts,
            freed: deltas[i] as u64,
        })
        .collect();
    let remove = step_idx.len() < p.recurring_steps;

    // Free space with one-off steps taken out.
    let mut y = Vec::with_capacity(pts.len());
    let mut level = pts[0].avail as f64;
    y.push(level);
    for (i, d) in deltas.iter().enumerate() {
        if !(remove && step_idx.contains(&i)) {
            level += d;
        }
        y.push(level);
    }
    let t: Vec<f64> = pts.iter().map(|pt| pt.ts as f64).collect();
    let (slope, lo, hi) = theil_sen(&t, &y)?;
    // Slope is free bytes per second; consumption is its opposite.
    let per_day = -slope * DAY as f64;
    Some((
        Rate {
            window,
            points: pts.len(),
            per_day,
            low: -hi * DAY as f64,
            high: -lo * DAY as f64,
            steps_removed: if remove { step_idx.len() } else { 0 },
        },
        steps,
    ))
}

/// Median, 10th and 90th percentile of the pairwise slopes. Long series are
/// thinned evenly to bound the O(n²) cost.
fn theil_sen(t: &[f64], y: &[f64]) -> Option<(f64, f64, f64)> {
    const MAX_POINTS: usize = 500;
    let idx: Vec<usize> = if t.len() > MAX_POINTS {
        (0..MAX_POINTS)
            .map(|k| k * (t.len() - 1) / (MAX_POINTS - 1))
            .collect()
    } else {
        (0..t.len()).collect()
    };
    let mut slopes = Vec::with_capacity(idx.len() * idx.len() / 2);
    for (a, &i) in idx.iter().enumerate() {
        for &j in &idx[a + 1..] {
            let dt = t[j] - t[i];
            if dt > 0.0 {
                slopes.push((y[j] - y[i]) / dt);
            }
        }
    }
    if slopes.is_empty() {
        return None;
    }
    slopes.sort_by(f64::total_cmp);
    let q = |f: f64| slopes[((slopes.len() - 1) as f64 * f).round() as usize];
    Some((q(0.5), q(0.1), q(0.9)))
}

fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: f64 = 1e9;

    /// Hourly samples over `days`, free space from `f(hours)`.
    fn series(days: i64, f: impl Fn(f64) -> f64) -> Vec<SpacePoint> {
        series_every(days, HOUR, f)
    }

    /// Samples every `step` seconds over `days`, free space from `f(hours)`.
    fn series_every(days: i64, step: i64, f: impl Fn(f64) -> f64) -> Vec<SpacePoint> {
        (0..=days * DAY / step)
            .map(|k| {
                let h = (k * step) as f64 / HOUR as f64;
                SpacePoint {
                    ts: k * step,
                    total: 500_000_000_000,
                    avail: f(h).max(0.0) as u64,
                }
            })
            .collect()
    }

    /// Deterministic noise in ±amp.
    fn wobble(h: f64, amp: f64) -> f64 {
        ((h * 12.9898).sin() * 43758.5453).fract() * amp
    }

    #[test]
    fn steady_consumption() {
        // 100 GB free, 3 GB/day consumed, ±50 MB noise.
        let pts = series(14, |h| {
            100.0 * GB - 3.0 * GB * h / 24.0 + wobble(h, 0.05 * GB)
        });
        let f = forecast(&pts, &Params::default()).unwrap();
        let r = f.rate.unwrap();
        assert_eq!(r.window, 14 * DAY);
        assert!((r.per_day / GB - 3.0).abs() < 0.05, "{r:?}");
        let full = f.full_in.unwrap();
        let expect = f.avail as f64 / (3.0 * GB);
        assert!((full.days - expect).abs() < 0.5, "{full:?}");
        assert!(full.earliest <= full.days && full.days <= full.latest.unwrap());
        assert!(f.steps.is_empty());
        assert!(f.recent_full_in.is_none(), "no acceleration");
    }

    #[test]
    fn a_cleanup_is_a_step_not_a_trend() {
        // 2 GB/day, then on day 7 a cleanup frees 30 GB.
        let pts = series(14, |h| {
            let base = 60.0 * GB - 2.0 * GB * h / 24.0;
            if h >= 7.0 * 24.0 {
                base + 30.0 * GB
            } else {
                base
            }
        });
        let f = forecast(&pts, &Params::default()).unwrap();
        let r = f.rate.unwrap();
        assert!((r.per_day / GB - 2.0).abs() < 0.05, "{r:?}");
        assert_eq!(r.steps_removed, 1);
        assert_eq!(f.steps.len(), 1);
        assert!((f.steps[0].freed as f64 / GB - 30.0).abs() < 0.2);
        // Full in (current free) / 2 GB a day, not "never".
        let full = f.full_in.unwrap();
        assert!((full.days - f.avail as f64 / (2.0 * GB)).abs() < 0.5);
    }

    #[test]
    fn a_recurring_sawtooth_uses_the_net_rate() {
        // A cache grows 1 GB/day and is cleared every 3 days: net ≈ 0.
        let pts = series(14, |h| 80.0 * GB - 1.0 * GB * ((h / 24.0) % 3.0));
        let f = forecast(&pts, &Params::default()).unwrap();
        let r = f.rate.unwrap();
        assert_eq!(r.steps_removed, 0, "recurring steps are the pattern");
        assert!(r.per_day.abs() < 0.3 * GB, "{r:?}");
    }

    #[test]
    fn plateau_and_emptying_disks_never_fill() {
        let flat = series(14, |h| 50.0 * GB + wobble(h, 0.02 * GB));
        let f = forecast(&flat, &Params::default()).unwrap();
        assert!(f.rate.unwrap().per_day.abs() < 0.05 * GB);
        let emptying = series(14, |h| 50.0 * GB + 0.5 * GB * h / 24.0);
        let f = forecast(&emptying, &Params::default()).unwrap();
        assert!(f.rate.unwrap().per_day < 0.0);
        assert!(f.full_in.is_none());
    }

    #[test]
    fn acceleration_is_reported_separately() {
        // 1 GB/day for 14 days, then the last 6 hours at 40 GB/day. Sampled
        // every 5 minutes, as the store keeps the most recent day.
        let pts = series_every(14, 300, |h| {
            let end = 14.0 * 24.0;
            let slow = 100.0 * GB - GB * h / 24.0;
            if h > end - 6.0 {
                slow - 40.0 * GB * (h - (end - 6.0)) / 24.0
            } else {
                slow
            }
        });
        let f = forecast(&pts, &Params::default()).unwrap();
        let recent = f.recent.unwrap();
        assert_eq!(recent.window, 6 * HOUR);
        assert!(recent.per_day > 20.0 * GB, "{recent:?}");
        let soon = f.recent_full_in.unwrap();
        assert!(soon.days < f.full_in.unwrap().days / 2.0);
    }

    #[test]
    fn too_little_history_gives_no_rate() {
        let pts = series(0, |_| 10.0 * GB);
        let f = forecast(&pts, &Params::default()).unwrap();
        assert!(f.rate.is_none() && f.full_in.is_none());
        assert!(forecast(&[], &Params::default()).is_none());
    }

    #[test]
    fn short_history_uses_the_window_it_covers() {
        // Two days of data: 14-day window not covered, 3-day window is (≥ 50%).
        let pts = series(2, |h| 40.0 * GB - 4.0 * GB * h / 24.0);
        let f = forecast(&pts, &Params::default()).unwrap();
        assert_eq!(f.rate.unwrap().window, 3 * DAY);
    }
}
