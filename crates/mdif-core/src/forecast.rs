//! When will the disk be full?
//!
//! With a day of history or more, the answer is a distribution (see `forecast/sim.rs`):
//! free space becomes net consumption per clock hour, and a thousand futures
//! are drawn from the past by blocks of the day, recent days counting more,
//! each with its own mix of days so the uncertainty about the rate itself is
//! kept. The median time to full is the forecast, the 10th and 90th
//! percentiles its range, and the share of futures full within 1, 3 and 14
//! days is what alerts are made of.
//!
//! With less than a day, the rate is a Theil–Sen slope (median of all
//! pairwise slopes) of free space over several windows: robust to noise and
//! to the odd outlier, and without assumptions about the distribution. The
//! longest of those windows is also reported as the trend.
//!
//! Space that comes back in one go (a cleanup, a deleted snapshot, an emptied
//! Trash) is a *step*, not a trend: one or two of those are taken out, so
//! freeing 20 GB doesn't read as "the disk is shrinking" and the forecast
//! keeps reflecting how fast things fill if nothing is done. Steps that
//! *recur* (three or more, e.g. a cache an app clears every few days) are the
//! pattern itself and stay in, so a sawtooth is forecast by its net rate
//! instead of raising a false alarm; but only when what they free fills up
//! again in between (half of it at least), so an afternoon of cleaning, many
//! steps and little refill, stays a one-off.
//!
//! Space *taken* in one go (a film downloaded, a macOS update prepared) is
//! an *arrival*, and a one-off too unless arrivals that big recur: fewer
//! than three at least half its size over four weeks, and it isn't still
//! going on. Left in, a 30 GB download is drawn into every simulated day
//! with the weight of the day it happened, and a disk that fills 3 GB a day
//! reads as full tomorrow. Arrivals that recur (a disk that takes 30 GB at
//! a time every few days) are how it fills, and stay.
//!
//! The disk is full when its free space *and* what macOS gives back by
//! itself (purgeable: local snapshots, caches) are used up, so the time to
//! full counts both, as Finder's "available" does.
//!
//! Clean-ups done through MyDiskIsFull (`mdif clean`, `mdif tidy`) are
//! recorded, and [`forecast_with`] takes out the changes each one falls in
//! whatever their size, even below the step threshold or spread over several
//! samples, and never counts them as a pattern.

#[cfg(test)]
mod backtest;
mod sim;

use serde::Serialize;

use crate::store::{Cleanup, SpacePoint, TIERS};

pub const HOUR: i64 = 3_600;
pub const DAY: i64 = 86_400;

/// Windows tried, longest first. The longest one with enough data is the
/// trend; the shortest is reported as the recent rate.
pub const WINDOWS: [i64; 3] = [14 * DAY, 3 * DAY, 6 * HOUR];

#[derive(Debug, Clone)]
pub struct Params {
    /// Smallest jump in free space that counts as a step.
    pub step_min: u64,
    /// A step must also exceed this many times the typical change per sample.
    pub step_noise_factor: f64,
    /// Steps per window from which they count as a recurring pattern...
    pub recurring_steps: usize,
    /// ...if the disk also consumed at least this share of what they freed
    /// over the same time, net of them: a cache that is cleared refills; a
    /// burst of cleaning frees far more than fills in between.
    pub recurring_refill: f64,
    pub min_points: usize,
    /// Share of a window the data must cover for that window to be used.
    pub min_coverage: f64,
    /// Futures simulated.
    pub paths: usize,
    /// Days of history the simulation draws from.
    pub history_days: i64,
    /// Days over which steps are counted to tell one-off from recurring
    /// (the store keeps hourly rows for a month).
    pub recurrence_days: i64,
    /// A day this many days old counts half as much as today.
    pub half_life_days: f64,
    /// The days count as this share of their effective number when each
    /// path draws its mix of them: below 1, the rate is less certain than
    /// independent days would make it, since days aren't (habits change).
    pub concentration: f64,
    /// Hours copied together from one past day (a divisor of 24).
    pub block_hours: usize,
    /// A block may come from up to this many hours earlier or later in the
    /// day than the hours it stands for.
    pub phase_jitter: usize,
    /// Not full within this many days: not filling.
    pub horizon_days: i64,
    /// Gaps in the samples up to this long have their change spread over
    /// their hours; longer ones are unknown.
    pub max_gap_hours: i64,
    /// Known hours needed for the simulation; with fewer, the trend is the
    /// forecast.
    pub min_sim_hours: usize,
    /// Known hours needed before the forecast can raise an alert.
    pub alert_hours: usize,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            step_min: 1_000_000_000,
            step_noise_factor: 10.0,
            recurring_steps: 3,
            recurring_refill: 0.5,
            min_points: 8,
            min_coverage: 0.5,
            paths: 1000,
            history_days: 14,
            recurrence_days: 28,
            half_life_days: 5.0,
            concentration: 0.25,
            block_hours: 6,
            phase_jitter: 1,
            horizon_days: 60,
            max_gap_hours: 48,
            min_sim_hours: 24,
            alert_hours: 36,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Step {
    pub ts: i64,
    /// Bytes that came back.
    pub freed: u64,
}

/// Space taken in one go and left out of the forecast as a one-off.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Arrival {
    /// When it ended.
    pub ts: i64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Rate {
    /// Seconds of history it comes from.
    pub window: i64,
    /// Samples (trend) or known hours (simulation) it comes from.
    pub points: usize,
    /// Bytes consumed per day (negative: the disk is getting emptier).
    pub per_day: f64,
    /// The plausible range, as bytes/day: for the simulation, the 10th and
    /// 90th percentile of the futures' rates; for a trend, of the pairwise
    /// slopes.
    pub low: f64,
    pub high: f64,
    /// Whether steps in this window were taken out (one-off cleanups) or
    /// kept (a recurring pattern). One-off arrivals count too.
    pub steps_removed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct FullIn {
    /// The forecast: the median time to full (simulation), or at the
    /// forecast rate (trend).
    pub days: f64,
    /// The 10th percentile (simulation), or at the fastest plausible rate.
    pub earliest: f64,
    /// The 90th percentile, or at the slowest plausible rate; `None` if
    /// that isn't within the horizon (or isn't filling).
    pub latest: Option<f64>,
}

/// How the forecast was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    /// Simulated futures drawn from the history (a day of it or more).
    Simulation,
    /// A Theil–Sen trend, with less than a day of history.
    Trend,
}

/// Consumption (one-off steps taken out) over the last hour, day and week,
/// each as bytes a day: the hour's as the Theil–Sen slope of its samples,
/// the day's and the week's as the net change. `None` when the samples
/// don't cover it well enough (6 samples over 45 minutes; 18 of the last 24
/// hours; half of the last 7 days).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Paces {
    pub hour: Option<f64>,
    pub day: Option<f64>,
    pub week: Option<f64>,
}

/// The probability that the disk is full within 1, 3 and 14 days.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct FullBy {
    pub d1: f64,
    pub d3: f64,
    pub d14: f64,
}

impl FullBy {
    fn min(self, o: FullBy) -> FullBy {
        FullBy {
            d1: self.d1.min(o.d1),
            d3: self.d3.min(o.d3),
            d14: self.d14.min(o.d14),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Forecast {
    pub at: i64,
    pub avail: u64,
    /// What macOS gives back by itself before the disk is full (purgeable),
    /// counted in the time to full.
    pub purgeable: u64,
    pub method: Method,
    /// The forecast rate: the simulation's, or the trend.
    pub rate: Option<Rate>,
    /// The longest Theil–Sen window with enough data.
    pub trend: Option<Rate>,
    /// The shortest Theil–Sen window, when it differs from the trend.
    pub recent: Option<Rate>,
    pub full_in: Option<FullIn>,
    /// Simulation only.
    pub full_by: Option<FullBy>,
    /// When the recent rate would fill the disk much sooner (> 2× faster).
    pub recent_full_in: Option<FullIn>,
    pub paces: Paces,
    /// Steps seen in the history the forecast comes from, oldest first.
    pub steps: Vec<Step>,
    /// One-off arrivals left out of it, oldest first.
    pub arrivals: Vec<Arrival>,
    /// What alerts are made of: `full_by`, at its lowest with any one day of
    /// the history left out. `None` until there's enough history.
    #[serde(skip)]
    pub robust: Option<FullBy>,
}

pub fn forecast(points: &[SpacePoint], p: &Params) -> Option<Forecast> {
    forecast_with(points, &Facts::default(), p)
}

/// What's known besides the free-space history.
#[derive(Debug, Clone, Copy, Default)]
pub struct Facts<'a> {
    /// Clean-ups done through MyDiskIsFull, taken out as steps whatever
    /// their size: every change in free space one of them falls in (see
    /// [`known`]) is left out, like a one-off step, and never counts towards
    /// a pattern. One that freed nothing by its own measure is ignored.
    pub cleanups: &'a [Cleanup],
    /// What macOS would free by itself now, which the disk uses up too
    /// before it's full.
    pub purgeable: u64,
    /// When the macOS update waiting for a restart was prepared: the
    /// arrival it falls in is a one-off whatever its size.
    pub update: Option<i64>,
}

/// The forecast, with what else is known taken into account.
pub fn forecast_with(points: &[SpacePoint], facts: &Facts, p: &Params) -> Option<Forecast> {
    let last = *points.last()?;
    let purgeable = facts.purgeable;
    let room = last.avail + purgeable;
    let cleanups: Vec<Cleanup> = facts
        .cleanups
        .iter()
        .filter(|c| c.freed > 0)
        .copied()
        .collect();
    let cleanups = cleanups.as_slice();
    let update = facts.update;
    let rates: Vec<Window> = WINDOWS
        .iter()
        .filter_map(|&w| window_rate(points, cleanups, update, last.ts - w, w, p))
        .collect();
    let trend = rates.first().map(|w| w.rate);
    let recent = rates
        .last()
        .map(|w| w.rate)
        .filter(|r| Some(r.window) != trend.map(|x| x.window));
    let hourly = sim::Hourly::with_cleanups(points, cleanups, update, p);
    let paces = hourly.as_ref().map_or(Paces::default(), |h| Paces {
        hour: h.last_hour(points, cleanups, p),
        day: h.mean_over(24, 0.75).map(|v| v * 24.0),
        week: h.mean_over(7 * 24, 0.5).map(|v| v * 24.0),
    });
    let first = rates.into_iter().next();
    let mut f = Forecast {
        at: last.ts,
        avail: last.avail,
        purgeable,
        method: Method::Trend,
        rate: trend,
        trend,
        recent,
        full_in: trend.and_then(|r| full_in(room, &r)),
        full_by: None,
        recent_full_in: None,
        paces,
        steps: first.as_ref().map(|w| w.steps.clone()).unwrap_or_default(),
        arrivals: first.map(|w| w.arrivals).unwrap_or_default(),
        robust: None,
    };
    if let Some((h, o)) = hourly.and_then(|h| simulate(&h, room, last.ts, p).map(|o| (h, o))) {
        f.method = Method::Simulation;
        f.rate = Some(o.rate);
        f.full_in = o.full_in;
        f.full_by = Some(o.full_by);
        f.robust = o.robust;
        f.steps = merged(h.steps, h.known);
        f.arrivals = h.arrivals;
    }
    f.recent_full_in = match (f.recent, f.rate) {
        (Some(rc), Some(r)) if rc.per_day > 2.0 * r.per_day.max(0.0) => full_in(room, &rc),
        _ => None,
    };
    Some(f)
}

/// A window's trend, and the steps and one-off arrivals in it.
struct Window {
    rate: Rate,
    steps: Vec<Step>,
    arrivals: Vec<Arrival>,
}

/// What the simulation says.
struct Outlook {
    rate: Rate,
    full_in: Option<FullIn>,
    full_by: FullBy,
    robust: Option<FullBy>,
}

/// The futures from `room` bytes left (free and purgeable) at `at`.
fn simulate(h: &sim::Hourly, room: u64, at: i64, p: &Params) -> Option<Outlook> {
    let pool = sim::Pool::new(h, p, None, p.min_sim_hours)?;
    let avail = room as f64;
    let seed = sim::seed(h, avail, at);
    let out = pool.run(p.paths, (p.horizon_days * 24) as usize, Some(avail), seed);
    let ttf = sim::sorted(out.ttf);
    let rates = sim::sorted(out.rate);
    let full_by = sim::full_by(&ttf);
    let days = |q: f64| sim::quantile(&ttf, q) / 24.0;
    let full_in = days(0.5).is_finite().then(|| FullIn {
        days: days(0.5),
        earliest: days(0.1),
        latest: Some(days(0.9)).filter(|d| d.is_finite()),
    });
    let known = h.known();
    let robust = (known >= p.alert_hours).then(|| {
        if full_by.d14 < ALERT_PROBABILITY {
            return full_by;
        }
        // Leave each day out in turn: an alert must not rest on one day.
        let mut least = full_by;
        for k in 0..h.x.len().div_ceil(24) {
            let by = match sim::Pool::new(h, p, Some(k), p.min_sim_hours / 2) {
                Some(pool) => {
                    let seed = seed ^ (k as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                    let out = pool.run(p.paths / 2, 14 * 24, Some(avail), seed);
                    sim::full_by(&out.ttf)
                }
                None => FullBy {
                    d1: 0.0,
                    d3: 0.0,
                    d14: 0.0,
                },
            };
            least = least.min(by);
        }
        least
    });
    Some(Outlook {
        rate: Rate {
            window: h.x.len() as i64 * HOUR,
            points: known,
            per_day: sim::quantile(&rates, 0.5),
            low: sim::quantile(&rates, 0.1),
            high: sim::quantile(&rates, 0.9),
            steps_removed: h.known.len()
                + h.arrivals.len()
                + if h.removed { h.steps.len() } else { 0 },
        },
        full_in,
        full_by,
        robust,
    })
}

/// How urgent the disk is, from least to most. Notifications escalate through
/// these, each level once per episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Alert {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "14_days")]
    Within14Days,
    #[serde(rename = "3_days")]
    Within3Days,
    #[serde(rename = "1_day")]
    WithinADay,
    /// Less than [`low_floor`] is free.
    #[serde(rename = "low")]
    Low,
}

/// Free space below this is an alert whatever the trend: the larger of
/// 10 GB and 2% of the volume.
pub fn low_floor(total: u64) -> u64 {
    10_000_000_000u64.max(total / 50)
}

/// A forecast level needs at least this probability of being full in time.
pub const ALERT_PROBABILITY: f64 = 0.5;

/// The alert for `avail` bytes free now and the forecast.
///
/// Being below the floor is a fact and needs no forecast. The forecast
/// levels (within 14 days, 3 days, a day) are raised when the disk is *more
/// likely than not* full in time (P ≥ 0.5, i.e. the median time to full is
/// within it), and only when that holds
/// - with a day and a half of history at least (the simulation needs a day;
///   alerts need more than one day to compare), and
/// - with any one day of the history left out in turn (a jackknife over
///   days): an alert never rests on a single day. An afternoon of builds,
///   or one big download, makes one day heavy; left out, the rest of the
///   history doesn't support the alert, so none is raised. A runaway filler
///   is an incident instead, and space actually running out is the floor's.
///
/// A sustained fill shows on every day and passes both. Notifications are
/// interruptions, so a miss here costs a day's warning at most (the floor
/// still alerts), while a false alarm costs trust in every later one.
pub fn alert(f: Option<&Forecast>, avail: u64, total: u64) -> Alert {
    alert_with(f, avail, low_floor(total), 14)
}

/// [`alert`] with the thresholds the user set (see [`crate::alerts`]): less
/// than `floor` free is [`Alert::Low`], and no forecast level further out
/// than `within_days` (1, 3 or 14) is raised. The rest is as above.
pub fn alert_with(f: Option<&Forecast>, avail: u64, floor: u64, within_days: u32) -> Alert {
    if avail < floor {
        return Alert::Low;
    }
    match f.and_then(|f| f.robust) {
        Some(by) if by.d1 >= ALERT_PROBABILITY => Alert::WithinADay,
        Some(by) if by.d3 >= ALERT_PROBABILITY && within_days >= 3 => Alert::Within3Days,
        Some(by) if by.d14 >= ALERT_PROBABILITY && within_days >= 14 => Alert::Within14Days,
        _ => Alert::None,
    }
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
    cleanups: &[Cleanup],
    update: Option<i64>,
    since: i64,
    window: i64,
    p: &Params,
) -> Option<Window> {
    let pts: Vec<&SpacePoint> = points.iter().filter(|pt| pt.ts >= since).collect();
    if pts.len() < p.min_points {
        return None;
    }
    let span = pts[pts.len() - 1].ts - pts[0].ts;
    if (span as f64) < p.min_coverage * window as f64 {
        return None;
    }
    let bin = resolution_at(window);
    let pts = resample(&pts, bin);
    if pts.len() < p.min_points {
        return None;
    }

    // Changes between consecutive samples; positive = space came back.
    let deltas: Vec<f64> = pts
        .windows(2)
        .map(|w| w[1].avail as f64 - w[0].avail as f64)
        .collect();
    let cleaned: Vec<bool> = pts
        .windows(2)
        .map(|w| known(cleanups, w[0].ts, w[1].ts, bin))
        .collect();
    let rest: Vec<f64> = (0..deltas.len())
        .filter(|&i| !cleaned[i])
        .map(|i| deltas[i])
        .collect();
    let (threshold, remove) = step_rule(&rest, p);
    let step_idx: Vec<usize> = (0..deltas.len())
        .filter(|&i| !cleaned[i] && deltas[i] >= threshold)
        .collect();
    let steps: Vec<Step> = step_idx
        .iter()
        .map(|&i| Step {
            ts: pts[i + 1].ts,
            freed: deltas[i] as u64,
        })
        .collect();
    let known_steps = runs(&cleaned, |i| deltas[i], |i| pts[i + 1].ts);
    let came = arrivals(deltas.len(), |i| -deltas[i], |i| cleaned[i], threshold);
    let calm = pts.partition_point(|pt| pt.ts < pts[pts.len() - 1].ts - HOUR);
    let one_off = one_off_arrivals(&came, calm.saturating_sub(1), p, |i| {
        update.is_some_and(|t| known_at(t, pts[i].ts, pts[i + 1].ts, bin))
    });

    // Free space with one-off steps, arrivals and clean-ups taken out.
    let mut y = Vec::with_capacity(pts.len());
    let mut level = pts[0].avail as f64;
    y.push(level);
    for (i, d) in deltas.iter().enumerate() {
        if !(cleaned[i] || (remove && step_idx.contains(&i)) || within(&one_off, i)) {
            level += d;
        }
        y.push(level);
    }
    let t: Vec<f64> = pts.iter().map(|pt| pt.ts as f64).collect();
    let (slope, lo, hi) = theil_sen(&t, &y)?;
    // Slope is free bytes per second; consumption is its opposite.
    let per_day = -slope * DAY as f64;
    Some(Window {
        rate: Rate {
            window,
            points: pts.len(),
            per_day,
            low: -hi * DAY as f64,
            high: -lo * DAY as f64,
            steps_removed: known_steps.len()
                + one_off.len()
                + if remove { step_idx.len() } else { 0 },
        },
        steps: merged(steps, known_steps),
        arrivals: one_off
            .iter()
            .map(|a| Arrival {
                ts: pts[a.last + 1].ts,
                bytes: a.bytes as u64,
            })
            .collect(),
    })
}

/// Space taken in one go: a run of consecutive changes that each took at
/// least the step threshold (a download over several samples is one).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Taken {
    /// Indices of its first and last change.
    pub first: usize,
    pub last: usize,
    pub bytes: f64,
}

/// The arrivals among `n` changes: `taken(i)` is what change `i` took
/// (negative: freed), `skip(i)` whether it's a clean-up, left out anyway.
pub(crate) fn arrivals(
    n: usize,
    taken: impl Fn(usize) -> f64,
    skip: impl Fn(usize) -> bool,
    threshold: f64,
) -> Vec<Taken> {
    let mut out: Vec<Taken> = Vec::new();
    for i in (0..n).filter(|&i| !skip(i) && taken(i) >= threshold) {
        match out.last_mut() {
            Some(a) if a.last + 1 == i => {
                a.last = i;
                a.bytes += taken(i);
            }
            _ => out.push(Taken {
                first: i,
                last: i,
                bytes: taken(i),
            }),
        }
    }
    out
}

/// The one-offs among `arrivals`: those with fewer than
/// [`Params::recurring_steps`] arrivals at least half as big (themselves
/// included) that ended before change `calm`, the start of the last hour
/// and the hour so far. One that hasn't been followed by an hour of calm may
/// be a runaway, and is what's happening now. And those with a change whose
/// cause is `known` to be a one-off (a macOS update), whatever their size.
pub(crate) fn one_off_arrivals(
    arrivals: &[Taken],
    calm: usize,
    p: &Params,
    known: impl Fn(usize) -> bool,
) -> Vec<Taken> {
    let rare = |a: &Taken| {
        arrivals.iter().filter(|b| b.bytes >= a.bytes / 2.0).count() < p.recurring_steps
    };
    arrivals
        .iter()
        .filter(|a| (a.last < calm && rare(a)) || (a.first..=a.last).any(&known))
        .copied()
        .collect()
}

/// Whether change `i` is part of one of `arrivals`.
pub(crate) fn within(arrivals: &[Taken], i: usize) -> bool {
    arrivals.iter().any(|a| (a.first..=a.last).contains(&i))
}

/// Whether `n` steps that freed `freed` bytes in all are a pattern, kept in
/// the forecast, rather than one-offs: [`Params::recurring_steps`] of them
/// at least, and refilled: what the disk consumed over the same time, net of
/// them (`refilled`), is at least [`Params::recurring_refill`] of what they
/// freed.
pub(crate) fn recurs(n: usize, freed: f64, refilled: f64, p: &Params) -> bool {
    n >= p.recurring_steps && refilled >= p.recurring_refill * freed
}

/// The steps among `deltas`, changes in free space between consecutive
/// samples (positive: space came back): the smallest change that is one (at
/// least [`Params::step_min`], and [`Params::step_noise_factor`] times the
/// median change), and whether they are one-offs, to be taken out.
pub(crate) fn step_rule(deltas: &[f64], p: &Params) -> (f64, bool) {
    let mut abs: Vec<f64> = deltas.iter().map(|d| d.abs()).collect();
    let noise = median(&mut abs).unwrap_or(0.0);
    let threshold = (p.step_min as f64).max(p.step_noise_factor * noise);
    let (mut n, mut freed) = (0, 0.0);
    for &d in deltas.iter().filter(|&&d| d >= threshold) {
        n += 1;
        freed += d;
    }
    let refilled = freed - deltas.iter().sum::<f64>();
    (threshold, !recurs(n, freed, refilled, p))
}

/// Whether a clean-up done through MyDiskIsFull may fall in the change of
/// free space from the level at `from` to the level at `to`. With `res` 0
/// those are samples, at their own times. Otherwise they are buckets of
/// `res` seconds starting then, each holding the level of its last sample
/// (what compaction keeps), which may have been taken anywhere in it: so a
/// clean-up marks the change into its own bucket and into the next one. That
/// depends on nothing compaction drops, so it decides the same before and
/// after.
pub(crate) fn known(cleanups: &[Cleanup], from: i64, to: i64, res: i64) -> bool {
    cleanups
        .iter()
        .any(|c| c.end.max(c.start) > from && c.start < to + res.max(1))
}

/// The same for a moment `t` (see [`known`]).
pub(crate) fn known_at(t: i64, from: i64, to: i64, res: i64) -> bool {
    t > from && t < to + res.max(1)
}

/// Runs of consecutive changes that clean-ups fall in, as one step each:
/// what came back over the run (`change(i)`, positive: space came back), at
/// the end of the run (`ts(i)`). A run that freed nothing is no step.
pub(crate) fn runs(
    cleaned: &[bool],
    change: impl Fn(usize) -> f64,
    ts: impl Fn(usize) -> i64,
) -> Vec<Step> {
    let mut out = Vec::new();
    let mut run: Option<(f64, i64)> = None;
    for (i, &c) in cleaned.iter().chain([&false]).enumerate() {
        if c {
            let r = run.get_or_insert((0.0, 0));
            r.0 += change(i);
            r.1 = ts(i);
        } else if let Some((freed, at)) = run.take()
            && freed > 0.0
        {
            out.push(Step {
                ts: at,
                freed: freed as u64,
            });
        }
    }
    out
}

/// Two lists of steps as one, oldest first.
fn merged(mut a: Vec<Step>, b: Vec<Step>) -> Vec<Step> {
    a.extend(b);
    a.sort_by_key(|s| s.ts);
    a
}

/// The coarsest resolution the store keeps for data up to `age` old.
fn resolution_at(age: i64) -> i64 {
    TIERS
        .iter()
        .find(|t| age <= t.keep)
        .unwrap_or(&TIERS[TIERS.len() - 1])
        .res
}

/// One point per `bin` seconds, holding the last level of the bucket, at
/// the bucket's start: exactly what compaction stores.
///
/// Every slope then weighs time evenly. Without this, a 3-day window would
/// hold the most recent day as 5-minute rows and the two before as hourly
/// rows, so the last day would outweigh them 12 to 1, and compaction would
/// change the forecast just by running.
fn resample(pts: &[&SpacePoint], bin: i64) -> Vec<SpacePoint> {
    let mut out: Vec<SpacePoint> = Vec::new();
    for pt in pts {
        let ts = pt.ts.div_euclid(bin) * bin;
        match out.last_mut() {
            Some(last) if last.ts == ts => *last = SpacePoint { ts, ..**pt },
            _ => out.push(SpacePoint { ts, ..**pt }),
        }
    }
    out
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
    fn a_one_off_download_is_not_a_rate() {
        // 2 GB/day, and on day 10 a 30 GB film in one hour: the forecast is
        // still 2 GB a day, not a 30 GB day drawn again and again.
        let pts = hourly(14, 150.0 * GB, |k| {
            let film = k == 10 * 24 + 15;
            (2.0 * GB / 24.0 + if film { 30.0 * GB } else { 0.0 }, true)
        });
        let f = forecast(&pts, &Params::default()).unwrap();
        let r = f.rate.unwrap();
        assert!((r.per_day / GB - 2.0).abs() < 0.3, "{r:?}");
        assert_eq!(f.arrivals.len(), 1, "{f:?}");
        assert!((f.arrivals[0].bytes as f64 / GB - 30.0).abs() < 0.2);
        assert!(f.full_by.unwrap().d1 < 0.05, "{f:?}");
    }

    #[test]
    fn arrivals_that_recur_are_how_the_disk_fills() {
        // 1 GB/day, and 20 GB more every other day: kept.
        let pts = hourly(14, 400.0 * GB, |k| {
            let big = k % 48 == 30;
            (GB / 24.0 + if big { 20.0 * GB } else { 0.0 }, true)
        });
        let f = forecast(&pts, &Params::default()).unwrap();
        assert!(f.arrivals.is_empty(), "{:?}", f.arrivals);
        assert!(f.rate.unwrap().per_day > 6.0 * GB, "{:?}", f.rate);
    }

    #[test]
    fn an_arrival_in_the_last_hour_stays_until_it_calms() {
        // 30 GB in the last full hour: it may be a runaway starting.
        let pts = hourly(7, 150.0 * GB, |k| {
            (
                GB / 24.0 + if k == 7 * 24 - 1 { 30.0 * GB } else { 0.0 },
                true,
            )
        });
        let f = forecast(&pts, &Params::default()).unwrap();
        assert!(f.arrivals.is_empty(), "{:?}", f.arrivals);
    }

    #[test]
    fn a_prepared_macos_update_is_a_one_off_whatever_its_peers() {
        // 25 GB arrivals every other day recur, but the one that was a
        // macOS update being prepared comes back when it's installed.
        let update = (12 * 24 + 6) as i64;
        let pts = hourly(14, 400.0 * GB, |k| {
            let big = k % 48 == 6;
            (GB / 24.0 + if big { 25.0 * GB } else { 0.0 }, true)
        });
        let at = update * HOUR - 600;
        let f = forecast_with(
            &pts,
            &Facts {
                update: Some(at),
                ..Facts::default()
            },
            &Params::default(),
        )
        .unwrap();
        assert_eq!(f.arrivals.len(), 1, "{:?}", f.arrivals);
        assert!(
            (f.arrivals[0].ts - update * HOUR).abs() <= HOUR,
            "{:?}",
            f.arrivals
        );
    }

    #[test]
    fn what_macos_frees_by_itself_counts_as_room() {
        let pts = series(7, |h| 20.0 * GB - 2.0 * GB * h / 24.0);
        let facts = |purgeable| Facts {
            purgeable,
            ..Facts::default()
        };
        let p = Params::default();
        let bare = forecast_with(&pts, &facts(0), &p).unwrap().full_in.unwrap();
        let f = forecast_with(&pts, &facts(40_000_000_000), &p).unwrap();
        let roomy = f.full_in.unwrap();
        // 6 GB free at 2 GB a day, then 40 GB more.
        assert!((bare.days - 3.0).abs() < 0.5, "{bare:?}");
        assert!((roomy.days - 23.0).abs() < 2.0, "{roomy:?}");
        assert_eq!((f.avail, f.purgeable), (6_000_000_000, 40_000_000_000));
    }

    #[test]
    fn a_recurring_sawtooth_uses_the_net_rate() {
        // A cache grows 1 GB/day and is cleared every 3 days: net ≈ 0.
        let pts = series(14, |h| 80.0 * GB - 1.0 * GB * ((h / 24.0) % 3.0));
        let f = forecast(&pts, &Params::default()).unwrap();
        let r = f.rate.unwrap();
        assert_eq!(r.steps_removed, 0, "recurring steps are the pattern");
        assert!(r.per_day.abs() < 0.5 * GB, "{r:?}");
        assert!(r.low < 0.0 && r.high > 0.0, "{r:?}");
        assert!(f.full_in.is_none(), "{:?}", f.full_in);
        // And the trend, as before.
        assert!(f.trend.unwrap().per_day.abs() < 0.3 * GB);
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
    fn compaction_does_not_change_the_forecast() {
        // Two flat days, then a day consuming 10 GB, sampled every 5 minutes.
        let pts = series_every(3, 300, |h| {
            if h < 48.0 {
                100.0 * GB
            } else {
                100.0 * GB - 10.0 * GB * (h - 48.0) / 24.0
            }
        });
        let raw = forecast(&pts, &Params::default()).unwrap();

        let store = crate::store::Store::open_in_memory().unwrap();
        for pt in &pts {
            store.add_space("/", *pt).unwrap();
        }
        store.compact(pts.last().unwrap().ts).unwrap();
        let compacted = store.space_since("/", 0).unwrap();
        assert!(compacted.len() < pts.len() / 2, "compaction ran");
        let after = forecast(&compacted, &Params::default()).unwrap();

        let (a, b) = (raw.rate.unwrap(), after.rate.unwrap());
        assert_eq!((a.window, b.window), (3 * DAY, 3 * DAY));
        assert!((a.per_day - b.per_day).abs() < 1e-6 * GB, "{a:?} vs {b:?}");
        assert_eq!(a.points, b.points);
        // The speed-up of the last day is the recent rate's job.
        let recent = after.recent.unwrap();
        assert!((recent.per_day / GB - 10.0).abs() < 0.5, "{recent:?}");
        assert!(after.recent_full_in.is_some());
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
        assert_eq!(f.trend.unwrap().window, 3 * DAY);
        // A day or more: simulated, from the two days there are.
        assert_eq!(f.method, Method::Simulation);
        assert_eq!(f.rate.unwrap().window, 2 * DAY);
        // Less than a day: the trend is the forecast.
        let pts = series_every(1, 300, |h| 40.0 * GB - 4.0 * GB * h / 24.0);
        let f = forecast(&pts[..200], &Params::default()).unwrap();
        assert_eq!(f.method, Method::Trend);
        assert_eq!(f.rate, f.trend);
        assert!(f.full_by.is_none() && f.full_in.is_some());
    }

    #[test]
    fn alerts_escalate_with_the_forecast() {
        let at = |gb_per_day: f64| {
            let pts = series(2, |h| 100.0 * GB - gb_per_day * GB * h / 24.0);
            let f = forecast(&pts, &Params::default()).unwrap();
            alert(Some(&f), f.avail, pts[0].total)
        };
        assert_eq!(at(1.0), Alert::None); // 98 GB left: full in 98 days
        assert_eq!(at(10.0), Alert::Within14Days); // 80 GB: 8 days
        assert_eq!(at(30.0), Alert::Within3Days); // 40 GB: 1.3 days
        assert_eq!(at(40.0), Alert::WithinADay); // 20 GB: half a day
        assert!(Alert::Low > Alert::WithinADay && Alert::Within14Days > Alert::None);
        // Levels further out than the user wants are not raised.
        let pts = series(2, |h| 100.0 * GB - 10.0 * GB * h / 24.0);
        let f = forecast(&pts, &Params::default()).unwrap();
        let floor = low_floor(pts[0].total);
        assert_eq!(
            alert_with(Some(&f), f.avail, floor, 14),
            Alert::Within14Days
        );
        assert_eq!(alert_with(Some(&f), f.avail, floor, 3), Alert::None);
        assert_eq!(alert_with(Some(&f), f.avail, 90 * GB as u64, 3), Alert::Low);
    }

    #[test]
    fn an_afternoon_of_builds_raises_no_forecast_alert() {
        // 6 hours at 100 GB/day with 60 GB left: the 6-hour window only.
        let pts: Vec<SpacePoint> = series_every(1, 300, |h| 85.0 * GB - 100.0 * GB * h / 24.0)
            .into_iter()
            .take(73)
            .collect();
        let f = forecast(&pts, &Params::default()).unwrap();
        assert_eq!(f.rate.unwrap().window, 6 * HOUR);
        assert!(f.full_in.unwrap().days < 1.0);
        assert_eq!(alert(Some(&f), f.avail, 500_000_000_000), Alert::None);
    }

    #[test]
    fn low_space_alerts_without_a_trend() {
        assert_eq!(low_floor(500_000_000_000), 10_000_000_000);
        assert_eq!(low_floor(2_000_000_000_000), 40_000_000_000);
        assert_eq!(alert(None, 9_000_000_000, 500_000_000_000), Alert::Low);
        assert_eq!(alert(None, 11_000_000_000, 500_000_000_000), Alert::None);
    }

    /// Hourly samples over `days` from `start` bytes free, `per_hour(k)`
    /// consumed in the hour ending at hour `k`; `None` for no sample then.
    fn hourly(days: i64, start: f64, per_hour: impl Fn(i64) -> (f64, bool)) -> Vec<SpacePoint> {
        let mut level = start;
        let mut out = Vec::new();
        for k in 0..=days * 24 {
            let (used, seen) = if k == 0 { (0.0, true) } else { per_hour(k) };
            level -= used;
            if seen {
                out.push(SpacePoint {
                    ts: k * HOUR,
                    total: 500_000_000_000,
                    avail: level.max(0.0) as u64,
                });
            }
        }
        out
    }

    #[test]
    fn the_time_to_full_is_a_distribution() {
        let pts = series(14, |h| {
            100.0 * GB - 3.0 * GB * h / 24.0 + wobble(h, 0.3 * GB)
        });
        let f = forecast(&pts, &Params::default()).unwrap();
        assert_eq!(f.method, Method::Simulation);
        let full = f.full_in.unwrap();
        let latest = full.latest.unwrap();
        assert!(full.earliest < full.days && full.days < latest, "{full:?}");
        assert!(latest - full.earliest < 0.4 * full.days, "{full:?}");
        let by = f.full_by.unwrap();
        assert!(by.d1 <= by.d3 && by.d3 <= by.d14, "{by:?}");
        assert_eq!(by.d1, 0.0);
        // The same history always gives the same forecast.
        assert_eq!(forecast(&pts, &Params::default()).unwrap(), f);
    }

    #[test]
    fn a_daily_rhythm_fills_the_disk_by_day() {
        // 1.2 GB an hour from 9:00 to 19:00, nothing at night: 12 GB a day,
        // with 60 GB left at midnight.
        let work = |k: i64| (10..=19).contains(&(k % 24));
        let pts = hourly(14, 228.0 * GB, |k| {
            (if work(k) { 1.2 * GB } else { 0.0 }, true)
        });
        let f = forecast(&pts, &Params::default()).unwrap();
        let full = f.full_in.unwrap();
        // Full during the fifth working day, not at a steady 12 GB a day
        // (exactly 5 days).
        assert!((4.6..4.85).contains(&full.days), "{full:?}");
        let hour_of_day = (f.at as f64 / HOUR as f64 + full.days * 24.0) % 24.0;
        assert!((9.0..=19.0).contains(&hour_of_day), "{hour_of_day}");
        assert!((f.rate.unwrap().per_day / GB - 12.0).abs() < 0.5);
    }

    #[test]
    fn a_burst_after_idle_days_shows_sooner_than_in_the_trend() {
        // 12 idle days, then two at 20 GB a day.
        let pts = hourly(14, 140.0 * GB, |k| {
            (if k > 12 * 24 { 20.0 * GB / 24.0 } else { 0.0 }, true)
        });
        let f = forecast(&pts, &Params::default()).unwrap();
        let (rate, trend) = (f.rate.unwrap(), f.trend.unwrap());
        assert!(trend.per_day < 1.0 * GB, "{trend:?}");
        assert!(rate.per_day > 3.0 * GB, "{rate:?}");
        assert!(rate.high > 10.0 * GB, "it may be the new normal: {rate:?}");
        assert!(f.full_in.is_some());
    }

    #[test]
    fn a_weekly_cleanup_is_a_pattern_once_seen_three_times() {
        // 3 GB a day, 18 GB freed every 7 days: 3/7 GB a day net.
        let pts = |days| {
            hourly(days, 200.0 * GB, |k| {
                let cleanup = k % (7 * 24) == 7 * 24 - 5;
                (
                    3.0 * GB / 24.0 - if cleanup { 18.0 * GB } else { 0.0 },
                    true,
                )
            })
        };
        let f = forecast(&pts(28), &Params::default()).unwrap();
        let r = f.rate.unwrap();
        assert_eq!((r.steps_removed, f.steps.len()), (0, 2), "{f:?}");
        assert!(r.per_day < 1.5 * GB, "{r:?}");
        // Seen twice: still a one-off, taken out.
        let f = forecast(&pts(14), &Params::default()).unwrap();
        let r = f.rate.unwrap();
        assert_eq!(r.steps_removed, 2);
        assert!((r.per_day / GB - 3.0).abs() < 0.3, "{r:?}");
    }

    #[test]
    fn gaps_are_not_zero_and_sleep_consumes_nothing() {
        // Awake 8:00–24:00 at 1 GB an hour (16 GB a day), asleep and
        // unsampled at night; the watcher off for three days in the middle.
        let asleep = |k: i64| (1..=8).contains(&(k % 24));
        let off = |k: i64| (4 * 24..7 * 24).contains(&k);
        let pts = hourly(10, 400.0 * GB, |k| {
            (if asleep(k) { 0.0 } else { GB }, !asleep(k) && !off(k))
        });
        let p = Params::default();
        let h = sim::Hourly::new(&pts, &p).unwrap();
        // Nights are spread (and measured nothing); the long gap is unknown.
        let unknown = h.x.iter().filter(|v| v.is_none()).count();
        assert!((60..=80).contains(&unknown), "{unknown}");
        let f = forecast(&pts, &p).unwrap();
        let r = f.rate.unwrap();
        assert!((r.per_day / GB - 16.0).abs() < 1.5, "{r:?}");
        assert!(
            (f.paces.day.unwrap() / GB - 16.0).abs() < 1.0,
            "{:?}",
            f.paces
        );
    }

    #[test]
    fn paces_cover_the_last_hour_day_and_week() {
        // 2 GB a day sampled every 5 minutes, and 20 GB cleaned up half an
        // hour ago: a one-off, so not a pace.
        let n = 10 * 24 * 12;
        let pts = series_every(10, 300, |h| {
            let base = 100.0 * GB - 2.0 * GB * h / 24.0;
            if h * 12.0 > (n - 6) as f64 {
                base + 20.0 * GB
            } else {
                base
            }
        });
        let f = forecast(&pts, &Params::default()).unwrap();
        let near = |v: Option<f64>| v.is_some_and(|v| (v / GB - 2.0).abs() < 0.2);
        assert!(
            near(f.paces.hour) && near(f.paces.day) && near(f.paces.week),
            "{:?}",
            f.paces
        );
        // Three days aren't a week.
        let f = forecast(&pts[n - 3 * 24 * 12..], &Params::default()).unwrap();
        assert!(f.paces.week.is_none() && near(f.paces.day));
    }

    #[test]
    fn one_heavy_day_raises_no_alert() {
        // Five quiet days, then 30 GB in the last one, with 20 GB left: the
        // simulation sees it can happen again, but no alert rests on a
        // single day (it's a download, or an incident).
        let pts = hourly(6, 50.0 * GB, |k| {
            (if k > 5 * 24 { 30.0 * GB / 24.0 } else { 0.0 }, true)
        });
        let f = forecast(&pts, &Params::default()).unwrap();
        assert!(f.full_by.unwrap().d14 >= 0.5, "{:?}", f.full_by);
        assert_eq!(alert(Some(&f), f.avail, 500_000_000_000), Alert::None);
        // The same fill every day does.
        let pts = hourly(6, 230.0 * GB, |_| (30.0 * GB / 24.0, true));
        let f = forecast(&pts, &Params::default()).unwrap();
        assert_eq!(
            alert(Some(&f), f.avail, 500_000_000_000),
            Alert::Within3Days
        );
    }

    /// Samples every 5 minutes over `days`: 200 GB free at first, `per_day`
    /// GB consumed a day, ±50 MB of noise, and each of `events` (hour,
    /// bytes) coming back from that hour on.
    fn with_events(days: i64, per_day: f64, events: &[(f64, f64)]) -> Vec<SpacePoint> {
        series_every(days, 300, |h| {
            let back: f64 = events.iter().filter(|e| h >= e.0 - 1e-9).map(|e| e.1).sum();
            200.0 * GB - per_day * GB * h / 24.0 + back + wobble(h, 0.05 * GB)
        })
    }

    /// A clean-up recorded just before the sample at `hour` (on the 5-minute
    /// grid), as `mdif clean` records it.
    fn cleanup_at(hour: f64, freed: f64) -> Cleanup {
        let ts = (hour * HOUR as f64).round() as i64;
        Cleanup {
            start: ts - 120,
            end: ts - 60,
            freed: freed as u64,
        }
    }

    #[test]
    fn an_afternoon_of_cleaning_is_a_one_off_however_many_steps() {
        // 4 GB a day for three days, and on the last one 68 GB cleaned up in
        // four goes over nine hours (some through the app, some in Finder;
        // none recorded): four steps, and nothing like 68 GB consumed
        // between them. Not a pattern: the forecast is the 4 GB a day.
        let events = [
            (60.0, 15.0 * GB),
            (63.0, 25.0 * GB),
            (66.5, 20.0 * GB),
            (69.0, 8.0 * GB),
        ];
        let pts = with_events(3, 4.0, &events);
        let f = forecast(&pts, &Params::default()).unwrap();
        let r = f.rate.unwrap();
        assert_eq!(f.method, Method::Simulation);
        assert!((r.per_day / GB - 4.0).abs() < 0.5, "{r:?}");
        assert_eq!((r.steps_removed, f.steps.len()), (4, 4), "{f:?}");
        let t = f.trend.unwrap();
        assert!((t.per_day / GB - 4.0).abs() < 0.5, "{t:?}");
        let day = f.paces.day.unwrap();
        assert!((day / GB - 4.0).abs() < 0.5, "{:?}", f.paces);
        // Counted as a pattern, as before, it read as the disk emptying.
        let by_count = Params {
            recurring_refill: f64::NEG_INFINITY,
            ..Params::default()
        };
        let old = forecast(&pts, &by_count).unwrap().rate.unwrap();
        assert!(old.per_day < 0.0 && old.steps_removed == 0, "{old:?}");
    }

    #[test]
    fn recorded_cleanups_come_out_even_below_the_step_threshold() {
        // 2 GB a day for two days, and on the second a clean-up through the
        // app every half hour for six hours, each freeing 0.4 GB: below the
        // step threshold every time and every hour, 4.8 GB in all.
        let events: Vec<(f64, f64)> = (0..12).map(|k| (40.0 + k as f64 / 2.0, 0.4 * GB)).collect();
        let pts = with_events(2, 2.0, &events);
        let p = Params::default();
        let without = forecast(&pts, &p).unwrap();
        assert!(without.steps.is_empty());
        assert!(without.rate.unwrap().per_day < 1.0 * GB, "{without:?}");

        let cleanups: Vec<Cleanup> = events.iter().map(|&(h, b)| cleanup_at(h, b)).collect();
        let f = forecast_with(
            &pts,
            &Facts {
                cleanups: &cleanups,
                ..Facts::default()
            },
            &p,
        )
        .unwrap();
        let r = f.rate.unwrap();
        assert!((r.per_day / GB - 2.0).abs() < 0.2, "{r:?}");
        // One step: the hours they fall in, 39:00 to 47:00, 4.8 GB less
        // what those eight hours consumed.
        assert_eq!((r.steps_removed, f.steps.len()), (1, 1), "{:?}", f.steps);
        assert_eq!(f.steps[0].ts, 46 * HOUR);
        let freed = f.steps[0].freed as f64 / GB;
        assert!((freed - (4.8 - 8.0 * 2.0 / 24.0)).abs() < 0.15, "{freed}");
        let near = |v: Option<f64>| v.is_some_and(|v| (v / GB - 2.0).abs() < 0.3);
        assert!(near(f.paces.day), "{:?}", f.paces);
        assert!(near(f.trend.map(|t| t.per_day)), "{:?}", f.trend);
        // What's free now is what's free now.
        assert_eq!((f.at, f.avail), (without.at, without.avail));
    }

    #[test]
    fn a_cleanup_spread_over_samples_is_one_step() {
        // 3 GB a day. In the last six hours: a clean-up that ran for half an
        // hour, its space coming back 0.9 GB at a time over six samples
        // (below the step threshold each), and two clean-ups between the
        // same two samples, 6 GB together.
        let mut events: Vec<(f64, f64)> = (1..=6)
            .map(|k| (43.0 + k as f64 / 12.0, 0.9 * GB))
            .collect();
        events.push((47.0, 6.0 * GB));
        let pts = with_events(2, 3.0, &events);
        let p = Params::default();
        // Unrecorded, the spread one is no step in the 5-minute samples,
        // and pulls the last six hours' rate down.
        let without = forecast(&pts, &p).unwrap().recent.unwrap();
        assert_eq!(without.steps_removed, 1, "{without:?}");
        assert!(without.per_day < 2.5 * GB, "{without:?}");

        let at = |h: f64| (h * HOUR as f64) as i64;
        let cleanups = [
            Cleanup {
                start: at(43.0) + 60,
                end: at(43.5) - 60,
                freed: 5_400_000_000,
            },
            Cleanup {
                start: at(47.0) - 240,
                end: at(47.0) - 200,
                freed: 2_500_000_000,
            },
            Cleanup {
                start: at(47.0) - 180,
                end: at(47.0) - 100,
                freed: 3_500_000_000,
            },
        ];
        let f = forecast_with(
            &pts,
            &Facts {
                cleanups: &cleanups,
                ..Facts::default()
            },
            &p,
        )
        .unwrap();
        // By the hour: each takes the hour it's in and the next (the hour's
        // level may come from before it), less what those hours consumed.
        let freed: Vec<f64> = f.steps.iter().map(|s| s.freed as f64 / GB).collect();
        assert_eq!(freed.len(), 2, "{:?}", f.steps);
        assert!(
            (freed[0] - 5.15).abs() < 0.15 && (freed[1] - 5.75).abs() < 0.15,
            "{freed:?}"
        );
        assert_eq!(
            f.steps.iter().map(|s| s.ts).collect::<Vec<_>>(),
            [44 * HOUR, 47 * HOUR]
        );
        // In the 5-minute samples of the last six hours, one step each too.
        let recent = f.recent.unwrap();
        assert_eq!(recent.steps_removed, 2);
        assert!((recent.per_day / GB - 3.0).abs() < 0.5, "{recent:?}");
        assert!((f.rate.unwrap().per_day / GB - 3.0).abs() < 0.3, "{f:?}");
    }

    #[test]
    fn deletions_in_the_last_hour_are_no_hourly_pace_on_a_disk_that_swings() {
        // Five-minute samples over two days. For the first 30 hours the disk
        // swings by 5 GB every hour, so no hourly change below 50 GB is a
        // step; then it fills 2 GB a day, quietly. In the last hour, three
        // deletions in Finder bring back 3 GB each: steps among its own
        // samples, and not refilled. (Unrecorded, the hour as a whole is
        // within what this disk's hours do, so it stays in the day's pace.)
        let back = [47.0 + 10.0 / 60.0, 47.0 + 25.0 / 60.0, 47.0 + 40.0 / 60.0];
        let pts = series_every(2, 300, |h| {
            let swing = if h < 30.0 && (h as i64) % 2 == 1 {
                5.0 * GB
            } else {
                0.0
            };
            let cleaned = back.iter().filter(|&&t| h >= t - 1e-9).count() as f64 * 3.0 * GB;
            100.0 * GB - 2.0 * GB * h / 24.0 + swing + cleaned
        });
        let p = Params::default();
        let h = sim::Hourly::new(&pts, &p).unwrap();
        assert!(
            h.threshold > 20.0 * GB && h.steps.is_empty(),
            "{}",
            h.threshold
        );
        let f = forecast(&pts, &p).unwrap();
        // The intervals with the deletions are left out whole, so a little
        // of the hour's consumption goes with them.
        let hour = f.paces.hour.unwrap();
        assert!((1.0..2.3).contains(&(hour / GB)), "{:?}", f.paces);
        let recent = f.recent.unwrap();
        assert!((recent.per_day / GB - 2.0).abs() < 0.3, "{recent:?}");
    }

    #[test]
    fn a_cleanup_before_the_mac_sleeps_still_comes_out() {
        // Awake 8:00–24:00 at 1 GB an hour, asleep and unsampled at night.
        // Just after the midnight sample on the second day, a clean-up
        // freed 20 GB, and the Mac went to sleep: the next sample is at 9:00.
        let asleep = |k: i64| (1..=8).contains(&(k % 24));
        let pts = hourly(4, 300.0 * GB, |k| {
            let cleaned = if k == 57 { 20.0 * GB } else { 0.0 };
            (if asleep(k) { 0.0 } else { GB } - cleaned, !asleep(k))
        });
        let cleanup = Cleanup {
            start: 48 * HOUR + 1800,
            end: 48 * HOUR + 1900,
            freed: 20_000_000_000,
        };
        let p = Params::default();
        let f = forecast_with(
            &pts,
            &Facts {
                cleanups: &[cleanup],
                ..Facts::default()
            },
            &p,
        )
        .unwrap();
        let r = f.rate.unwrap();
        assert!((r.per_day / GB - 16.0).abs() < 1.5, "{r:?}");
        // The night is no 20 GB coming back; the step is the hour before and
        // the night with the hour after it (1 GB used in each, 20 GB back).
        assert_eq!(f.steps.len(), 1, "{:?}", f.steps);
        assert_eq!(f.steps[0].ts, 57 * HOUR);
        assert!(
            (f.steps[0].freed as f64 / GB - 18.0).abs() < 0.01,
            "{:?}",
            f.steps
        );
        // Recorded before the history, or as freeing nothing: no change.
        let early = Cleanup {
            start: -500,
            end: -400,
            freed: GB as u64,
        };
        let nothing = Cleanup {
            freed: 0,
            ..cleanup
        };
        assert_eq!(
            forecast_with(
                &pts,
                &Facts {
                    cleanups: &[early, nothing],
                    ..Facts::default()
                },
                &p
            )
            .unwrap(),
            forecast(&pts, &p).unwrap()
        );
    }

    #[test]
    fn compaction_does_not_change_what_cleanups_take_out() {
        // Three days of 5-minute samples at 4 GB a day, with clean-ups on
        // the first day (folded into hourly rows by compaction), one of them
        // in the last minutes of an hour.
        let events = [
            (10.0, 3.0 * GB),
            (11.0 - 1.0 / 12.0, 0.5 * GB),
            (20.5, 6.0 * GB),
        ];
        let pts = with_events(3, 4.0, &events);
        let cleanups: Vec<Cleanup> = events.iter().map(|&(h, b)| cleanup_at(h, b)).collect();
        let raw = forecast_with(
            &pts,
            &Facts {
                cleanups: &cleanups,
                ..Facts::default()
            },
            &Params::default(),
        )
        .unwrap();
        let store = crate::store::Store::open_in_memory().unwrap();
        for pt in &pts {
            store.add_space("/", *pt).unwrap();
        }
        store.compact(pts.last().unwrap().ts).unwrap();
        let compacted = store.space_since("/", 0).unwrap();
        assert!(compacted.len() < pts.len() / 2, "compaction ran");
        let after = forecast_with(
            &compacted,
            &Facts {
                cleanups: &cleanups,
                ..Facts::default()
            },
            &Params::default(),
        )
        .unwrap();
        assert_eq!(raw.steps, after.steps);
        assert_eq!(raw.trend, after.trend);
        let (a, b) = (raw.rate.unwrap(), after.rate.unwrap());
        assert!((a.per_day - b.per_day).abs() < 1e-6 * GB, "{a:?} vs {b:?}");
        assert_eq!(a.steps_removed, 2);
        assert!((a.per_day / GB - 4.0).abs() < 0.3, "{a:?}");
    }

    #[test]
    fn a_day_and_a_half_of_history_before_any_forecast_alert() {
        // 30 hours at 40 GB a day with 50 GB left: simulated, not alerted.
        let pts = hourly(2, 100.0 * GB, |_| (40.0 * GB / 24.0, true));
        let f = forecast(&pts[..31], &Params::default()).unwrap();
        assert_eq!(f.method, Method::Simulation);
        assert!(f.full_in.unwrap().days < 2.0);
        assert_eq!(alert(Some(&f), f.avail, 500_000_000_000), Alert::None);
    }
}
