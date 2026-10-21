//! Time to full as a distribution, by simulation.
//!
//! Free space becomes net consumption per clock hour ([`Hourly`]), with
//! one-off cleanups taken out (they are interventions; the forecast is what
//! happens if nothing changes) and recurring ones kept (they are how the disk
//! behaves). Future hours are then drawn from the past ([`Pool`]):
//!
//! - **In blocks, by time of day.** The next hours are copied from the same
//!   hours of a past day (give or take [`Params::phase_jitter`] hours), a
//!   block of [`Params::block_hours`] at a time, so a day keeps its rhythm
//!   (builds by day, idle nights) and a burst keeps its length.
//! - **Recent days count more**, by an exponential half-life.
//! - **Each path draws its own past** (a Bayesian bootstrap: Dirichlet
//!   weights over the days). Drawing days independently would average them
//!   out over a long horizon and claim a precision the history doesn't
//!   have; drawing a different mix of days per path keeps the uncertainty
//!   about the rate itself, which is what makes the time-to-full range honest
//!   when there are only a few days to go on.
//!
//! Hours with no samples around them are unknown, not zero. A gap of a few
//! hours (the Mac asleep, the watcher stopped) has its net change spread
//! over its hours: it is measured, so nothing is invented, and a sleeping
//! Mac correctly consumes nothing. Longer gaps are unknown.
//!
//! Deterministic: the random numbers are seeded from the data.

use super::{
    DAY, FullBy, HOUR, Params, Step, WINDOWS, known, median, recurs, runs, step_rule, theil_sen,
};
use crate::store::{Cleanup, SpacePoint};

/// Free space as consumption per clock hour.
#[derive(Debug, Clone)]
pub(crate) struct Hourly {
    /// Bytes consumed during each clock hour, oldest first (negative: space
    /// came back). The last is the current hour so far. `None`: unknown.
    pub x: Vec<Option<f64>>,
    /// Space that came back in one go, oldest first.
    pub steps: Vec<Step>,
    /// Clean-ups done through MyDiskIsFull, oldest first: always taken out.
    pub known: Vec<Step>,
    /// Per hour of `x`: left out as a step or a clean-up. Not missing data,
    /// so it counts as covered for [`Hourly::mean_over`].
    pub stepped: Vec<bool>,
    /// Whether the steps were taken out: too few over
    /// [`Params::recurrence_days`] to be a pattern, or not refilled.
    pub removed: bool,
    /// Smallest change that counts as a step.
    pub threshold: f64,
}

impl Hourly {
    /// The last [`Params::history_days`] of `points`, by clock hour. Each
    /// hour's level is its last sample, which is what compaction keeps, so
    /// compacting the store changes nothing here.
    #[cfg(test)]
    pub fn new(points: &[SpacePoint], p: &Params) -> Option<Hourly> {
        Hourly::with_cleanups(points, &[], p)
    }

    /// The same, with the hours `cleanups` fall in left out (see
    /// [`known`](super::known)).
    pub fn with_cleanups(
        points: &[SpacePoint],
        cleanups: &[Cleanup],
        p: &Params,
    ) -> Option<Hourly> {
        let last = points.last()?.ts.div_euclid(HOUR);
        let first = last - p.history_days * 24;
        // Steps are told one-off or recurring over a longer stretch: a
        // weekly cleanup shows only twice in two weeks.
        let lookback = last - p.recurrence_days.max(p.history_days) * 24;
        let mut levels: Vec<(i64, f64)> = Vec::new();
        for pt in points {
            let b = pt.ts.div_euclid(HOUR);
            if b < lookback {
                continue;
            }
            match levels.last_mut() {
                Some(l) if l.0 == b => l.1 = pt.avail as f64,
                _ => levels.push((b, pt.avail as f64)),
            }
        }
        // (hour, hours since the previous level, bytes consumed since then,
        // whether a clean-up through MyDiskIsFull falls in it)
        let deltas: Vec<(i64, i64, f64, bool)> = levels
            .windows(2)
            .map(|w| {
                let cleaned = known(cleanups, w[0].0 * HOUR, w[1].0 * HOUR, HOUR);
                (w[1].0, w[1].0 - w[0].0, w[0].1 - w[1].1, cleaned)
            })
            .collect();
        let mut abs: Vec<f64> = deltas
            .iter()
            .filter(|d| d.1 == 1 && !d.3)
            .map(|d| d.2.abs())
            .collect();
        if abs.is_empty() {
            abs = deltas.iter().filter(|d| !d.3).map(|d| d.2.abs()).collect();
        }
        let noise = median(&mut abs).unwrap_or(0.0);
        let threshold = (p.step_min as f64).max(p.step_noise_factor * noise);
        let is_step = |consumed: f64| -consumed >= threshold;
        // Steps recur when there are enough of them and the disk refills
        // what they free (see `recurs`).
        let (mut n, mut freed, mut refilled) = (0, 0.0, 0.0);
        for d in deltas.iter().filter(|d| !d.3) {
            if is_step(d.2) {
                n += 1;
                freed -= d.2;
            } else {
                refilled += d.2;
            }
        }
        let removed = !recurs(n, freed, refilled, p);

        let levels: Vec<(i64, f64)> = levels.into_iter().filter(|l| l.0 >= first).collect();
        if levels.len() < 2 {
            return None;
        }
        let deltas = &deltas[deltas.len() - (levels.len() - 1)..];
        let steps: Vec<Step> = deltas
            .iter()
            .filter(|d| !d.3 && is_step(d.2))
            .map(|d| Step {
                ts: d.0 * HOUR,
                freed: (-d.2) as u64,
            })
            .collect();
        let cleaned: Vec<bool> = deltas.iter().map(|d| d.3).collect();
        let known = runs(&cleaned, |i| -deltas[i].2, |i| deltas[i].0 * HOUR);
        let start = levels[0].0 + 1;
        let mut x = vec![None; (levels[levels.len() - 1].0 - levels[0].0) as usize];
        let mut stepped = vec![false; x.len()];
        for &(hour, gap, consumed, cleaned) in deltas {
            let step = cleaned || (removed && is_step(consumed));
            if step || gap > p.max_gap_hours {
                if step && gap <= p.max_gap_hours {
                    for h in hour - gap + 1..=hour {
                        stepped[(h - start) as usize] = true;
                    }
                }
                continue;
            }
            for h in hour - gap + 1..=hour {
                x[(h - start) as usize] = Some(consumed / gap as f64);
            }
        }
        Some(Hourly {
            x,
            steps,
            known,
            stepped,
            removed,
            threshold,
        })
    }

    pub fn known(&self) -> usize {
        self.x.iter().filter(|v| v.is_some()).count()
    }

    /// Mean consumption per hour over the last `hours`, if at least
    /// `min_cover` of them are known (or left out as steps).
    pub fn mean_over(&self, hours: usize, min_cover: f64) -> Option<f64> {
        let from = self.x.len().saturating_sub(hours);
        let known: Vec<f64> = self.x[from..].iter().flatten().copied().collect();
        let covered = known.len() + self.stepped[from..].iter().filter(|&&s| s).count();
        if known.is_empty() || (covered as f64) < min_cover * hours as f64 {
            return None;
        }
        Some(known.iter().sum::<f64>() / known.len() as f64)
    }

    /// Consumption over the last hour, as bytes a day, with one-off steps
    /// taken out: the Theil–Sen slope of its samples, since a net change
    /// over an hour mostly measures the transient dip (a build's temporary
    /// files) at either end. `None` with fewer than 6 samples or less than
    /// 45 minutes of them.
    ///
    /// Steps are the hourly ones, and also those its own samples show: a
    /// rise that is a step among the changes of the last six hours (as for
    /// the recent rate), when those steps don't recur. On a disk whose hours
    /// swing by gigabytes, a few deletions in Finder are no hourly step, and
    /// would otherwise read as the disk emptying fast.
    pub fn last_hour(
        &self,
        points: &[SpacePoint],
        cleanups: &[Cleanup],
        p: &Params,
    ) -> Option<f64> {
        let last = points.last()?;
        let from = points.partition_point(|pt| pt.ts < last.ts - HOUR);
        let pts = &points[from..];
        if pts.len() < 6 || last.ts - pts[0].ts < 3 * HOUR / 4 {
            return None;
        }
        let cleaned = |a: &SpacePoint, b: &SpacePoint| known(cleanups, a.ts, b.ts, 0);
        let around = points.partition_point(|pt| pt.ts < last.ts - WINDOWS[WINDOWS.len() - 1]);
        let deltas: Vec<f64> = points[around..]
            .windows(2)
            .filter(|w| !cleaned(&w[0], &w[1]))
            .map(|w| w[1].avail as f64 - w[0].avail as f64)
            .collect();
        let (fine, one_off) = step_rule(&deltas, p);
        let mut y = Vec::with_capacity(pts.len());
        let mut level = pts[0].avail as f64;
        y.push(level);
        for w in pts.windows(2) {
            let d = w[1].avail as f64 - w[0].avail as f64;
            let step = cleaned(&w[0], &w[1])
                || (self.removed && d >= self.threshold)
                || (one_off && d >= fine);
            if !step {
                level += d;
            }
            y.push(level);
        }
        let t: Vec<f64> = pts.iter().map(|pt| pt.ts as f64).collect();
        let (slope, _, _) = theil_sen(&t, &y)?;
        Some(-slope * DAY as f64)
    }
}

/// A block of past hours a path can copy.
#[derive(Debug, Clone, Copy)]
struct Cand {
    /// The unit (a day, or a block) it starts in, 0 the most recent;
    /// `None`: the fallback.
    day: Option<usize>,
    /// Where its hours are in [`Pool::vals`].
    at: usize,
    total: f64,
    /// The most it consumes at any point inside the block.
    peak: f64,
}

/// What paths draw from.
pub(crate) struct Pool {
    block: usize,
    /// Per block of the day, the past blocks that match it.
    slots: Vec<Vec<Cand>>,
    vals: Vec<f64>,
    /// Dirichlet concentration of each unit (0: not drawn).
    alpha: Vec<f64>,
}

/// Results of [`Pool::run`], one per path.
pub(crate) struct Outcome {
    /// Hours until the disk is full; infinite if not within the horizon.
    pub ttf: Vec<f64>,
    /// Each path's expected consumption a day, under its own weights.
    pub rate: Vec<f64>,
    /// Consumed by the end of the horizon (or when full).
    pub consumed: Vec<f64>,
}

impl Pool {
    /// The pool, leaving out day `exclude` if given; `None` with fewer than
    /// `min_hours` known hours to draw from.
    pub fn new(h: &Hourly, p: &Params, exclude: Option<usize>, min_hours: usize) -> Option<Pool> {
        let n = h.x.len();
        let block = p.block_hours.clamp(1, 24);
        let block = (1..=block).rev().find(|b| 24 % b == 0).unwrap_or(1);
        let ndays = n.div_ceil(24);
        let day_of = |j: usize| (n - 1 - j) / 24;
        let recency = |k: usize| {
            if Some(k) == exclude {
                0.0
            } else {
                0.5f64.powf(k as f64 / p.half_life_days.max(1e-3))
            }
        };
        // The share of `len` hours before `hi` that are known.
        let cover = |hi: usize, len: usize| {
            let lo = hi.saturating_sub(len);
            h.x[lo..hi].iter().filter(|v| v.is_some()).count() as f64 / len as f64
        };
        let days: Vec<f64> = (0..ndays)
            .map(|k| recency(k) * cover(n - 24 * k, 24))
            .collect();
        // Days of data, counting partial ones by their share.
        let kdays = ((0..ndays)
            .filter(|&k| Some(k) != exclude)
            .map(|k| cover(n - 24 * k, 24))
            .sum::<f64>()
            .floor() as usize)
            .max(1);
        let usable = (0..n)
            .filter(|&j| h.x[j].is_some() && days[day_of(j)] > 0.0)
            .count();
        if usable < min_hours.max(1) {
            return None;
        }
        // With few days, how the day goes can't be told from noise: any hour
        // of the day stands for any other. The neighbourhood narrows to
        // `phase_jitter` as days accumulate (±11 hours with one day, ±5
        // with two, ±3 with three).
        let jitter = (p.phase_jitter as i64).max(12 / kdays as i64 - 1).min(11);
        // What each path weighs: days, which is what hours depend on
        // together; with fewer than three, blocks, so that a path's rate
        // still varies.
        let unit = if kdays >= 3 { 24 } else { block };
        let unit_of = |j: usize| (n - 1 - j) / unit;
        let w: Vec<f64> = (0..n.div_ceil(unit))
            .map(|u| recency(u * unit / 24) * cover(n - unit * u, unit))
            .collect();
        // Per-hour mean, for a block of the day nothing matches.
        let (mut num, mut den) = (0.0, 0.0);
        for (j, v) in h.x.iter().enumerate() {
            if let Some(v) = v {
                num += w[unit_of(j)] * v;
                den += w[unit_of(j)];
            }
        }
        let fallback = num / den;

        let mut vals = Vec::new();
        let mut slots = Vec::with_capacity(24 / block);
        for s in 0..24 / block {
            let mut cands = Vec::new();
            for m in 1i64.. {
                let base = (n as i64 - 1) + (s * block) as i64 + 1 - 24 * m;
                if base + jitter < 0 {
                    break;
                }
                for d in -jitter..=jitter {
                    let j0 = base + d;
                    if j0 < 0 || j0 as usize + block > n {
                        continue;
                    }
                    let j0 = j0 as usize;
                    let u = unit_of(j0);
                    if w[u] == 0.0 || days[day_of(j0)] == 0.0 {
                        continue;
                    }
                    let hours = &h.x[j0..j0 + block];
                    let known: Vec<f64> = hours.iter().flatten().copied().collect();
                    if known.len() * 2 < block {
                        continue;
                    }
                    let mean = known.iter().sum::<f64>() / known.len() as f64;
                    let at = vals.len();
                    vals.extend(hours.iter().map(|v| v.unwrap_or(mean)));
                    cands.push(block_of(&vals[at..], Some(u), at));
                }
            }
            if cands.is_empty() {
                let at = vals.len();
                vals.extend(std::iter::repeat_n(fallback, block));
                cands.push(block_of(&vals[at..], None, at));
            }
            slots.push(cands);
        }

        // Concentration: the effective number of units, shared by weight
        // (equal weights give the classic Dirichlet(1, …, 1)), tempered.
        let sum: f64 = w.iter().sum();
        let sum2: f64 = w.iter().map(|x| x * x).sum();
        let n_eff = sum * sum / sum2 * p.concentration;
        let alpha = w.iter().map(|x| n_eff * x / sum).collect();
        Some(Pool {
            block,
            slots,
            vals,
            alpha,
        })
    }

    /// `paths` futures of up to `horizon` hours, each stopping when its
    /// consumption reaches `avail` (if given).
    pub fn run(&self, paths: usize, horizon: usize, avail: Option<f64>, seed: u64) -> Outcome {
        let mut rng = Rng(seed);
        let mut g = vec![0.0; self.alpha.len()];
        let mut cum: Vec<Vec<f64>> = self
            .slots
            .iter()
            .map(|c| Vec::with_capacity(c.len()))
            .collect();
        let mut out = Outcome {
            ttf: Vec::with_capacity(paths),
            rate: Vec::with_capacity(paths),
            consumed: Vec::with_capacity(paths),
        };
        let limit = avail.unwrap_or(f64::INFINITY);
        for _ in 0..paths {
            for (gk, &a) in g.iter_mut().zip(&self.alpha) {
                *gk = if a > 0.0 { rng.gamma(a) } else { 0.0 };
            }
            let mut rate = 0.0;
            for (cands, cw) in self.slots.iter().zip(cum.iter_mut()) {
                cw.clear();
                let (mut acc, mut mean) = (0.0, 0.0);
                for c in cands {
                    let wt = c.day.map_or(1.0, |d| g[d]);
                    acc += wt;
                    mean += wt * c.total;
                    cw.push(acc);
                }
                if acc > 0.0 {
                    rate += mean / acc;
                } else {
                    // Every weight underflowed: draw evenly.
                    for (i, x) in cw.iter_mut().enumerate() {
                        *x = (i + 1) as f64;
                    }
                    rate += cands.iter().map(|c| c.total).sum::<f64>() / cands.len() as f64;
                }
            }
            out.rate.push(rate);

            let (mut used, mut t) = (0.0, 0usize);
            let mut ttf = if limit <= 0.0 { 0.0 } else { f64::INFINITY };
            'path: while t < horizon && ttf.is_infinite() {
                for (cands, cw) in self.slots.iter().zip(&cum) {
                    let u = rng.f64() * cw[cw.len() - 1];
                    let c = cands[cw.partition_point(|&x| x <= u).min(cands.len() - 1)];
                    let hours = &self.vals[c.at..c.at + self.block];
                    if used + c.peak < limit && t + self.block <= horizon {
                        used += c.total;
                        t += self.block;
                    } else {
                        for &v in hours {
                            if t >= horizon {
                                break 'path;
                            }
                            if used + v >= limit && v > 0.0 {
                                ttf = t as f64 + (limit - used) / v;
                                used = limit;
                                break 'path;
                            }
                            used += v;
                            t += 1;
                        }
                    }
                    if t >= horizon {
                        break 'path;
                    }
                }
            }
            out.ttf.push(ttf);
            out.consumed.push(used);
        }
        out
    }
}

fn block_of(hours: &[f64], day: Option<usize>, at: usize) -> Cand {
    let (mut total, mut peak) = (0.0, f64::NEG_INFINITY);
    for v in hours {
        total += v;
        peak = peak.max(total);
    }
    Cand {
        day,
        at,
        total,
        peak,
    }
}

/// The share of paths full within 1, 3 and 14 days.
pub(crate) fn full_by(ttf: &[f64]) -> FullBy {
    let share = |days: f64| {
        ttf.iter().filter(|&&t| t <= days * 24.0).count() as f64 / ttf.len().max(1) as f64
    };
    FullBy {
        d1: share(1.0),
        d3: share(3.0),
        d14: share(14.0),
    }
}

/// The `q` quantile of sorted `v`.
pub(crate) fn quantile(v: &[f64], q: f64) -> f64 {
    v[((v.len() - 1) as f64 * q).round() as usize]
}

pub(crate) fn sorted(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(f64::total_cmp);
    v
}

/// A seed from everything the forecast depends on, so the same history
/// always gives the same forecast.
pub(crate) fn seed(h: &Hourly, avail: f64, at: i64) -> u64 {
    // FNV-1a.
    let mut s: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |x: u64| {
        for b in x.to_le_bytes() {
            s ^= b as u64;
            s = s.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for v in &h.x {
        eat(v.map_or(u64::MAX, f64::to_bits));
    }
    eat(avail.to_bits());
    eat(at as u64);
    s
}

/// SplitMix64: small, fast and good enough for resampling.
pub(crate) struct Rng(pub u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn f64(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn normal(&mut self) -> f64 {
        let u = 1.0 - self.f64();
        let v = self.f64();
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }

    /// Gamma(shape, 1), by Marsaglia and Tsang (boosted below shape 1).
    pub fn gamma(&mut self, shape: f64) -> f64 {
        if shape < 1.0 {
            let u = 1.0 - self.f64();
            return self.gamma(shape + 1.0) * u.powf(1.0 / shape);
        }
        let d = shape - 1.0 / 3.0;
        let c = 1.0 / (9.0 * d).sqrt();
        loop {
            let x = self.normal();
            let v = 1.0 + c * x;
            if v <= 0.0 {
                continue;
            }
            let v = v * v * v;
            let u = 1.0 - self.f64();
            if u.ln() < 0.5 * x * x + d - d * v + d * v.ln() {
                return d * v;
            }
        }
    }
}
