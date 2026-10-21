//! Backtests: from each hour of a history, forecast what the disk consumes
//! over the next hours with the old method (the Theil–Sen trend and the
//! 10th–90th percentile of its pairwise slopes) and the new one (the
//! simulation's median and 10th–90th percentile), and compare with what
//! happened.
//!
//! Scored by the mean absolute error of the point forecast, the share of
//! outcomes inside the 10–90 band (80% if calibrated), its mean width, and
//! the interval score (Gneiting & Raftery 2007: width plus 10× the miss,
//! a proper score that rewards bands both narrow and right).
//!
//! The synthetic histories run with `cargo test`; the tuning grid and a
//! real history are ignored tests:
//!
//!     cargo test --release -p mdif-core backtest -- --ignored --nocapture
//!     MDIF_BACKTEST_DB=/path/to/copy.sqlite3 cargo test --release …
//!
//! The database is opened read-only; still, point it at a copy.

use super::sim::{self, Hourly, Pool, Rng};
use super::*;

const GB: f64 = 1e9;

/// One hour of a synthetic history.
#[derive(Default, Clone, Copy)]
struct Hour {
    /// What the disk consumed (negative: freed by itself, e.g. a cache an
    /// app clears). The truth the forecast is scored against.
    consumed: f64,
    /// Freed by an intervention (a cleanup by hand): in the samples, not in
    /// the truth.
    cleaned: f64,
    /// No sample at the end of this hour (asleep, watcher off).
    unseen: bool,
}

struct History {
    name: &'static str,
    points: Vec<SpacePoint>,
    /// Origins: (index into `points`, what was consumed over the horizon).
    origins: Vec<(usize, f64)>,
}

/// A synthetic history, and the truth behind it.
struct Synthetic {
    name: &'static str,
    points: Vec<SpacePoint>,
    consumed: Vec<f64>,
    /// Each hour's sample in `points`, if it has one.
    index: Vec<Option<usize>>,
}

impl Synthetic {
    /// Origins from each sampled hour after the first day, scored `hours`
    /// ahead.
    fn ahead(&self, hours: usize) -> History {
        let n = self.consumed.len();
        let origins = (24..n.saturating_sub(hours))
            .filter_map(|k| {
                let truth = self.consumed[k + 1..=k + hours].iter().sum();
                self.index[k].map(|i| (i, truth))
            })
            .collect();
        History {
            name: self.name,
            points: self.points.clone(),
            origins,
        }
    }
}

/// `days` of hours from `f(hour, rng)`, sampled once an hour.
fn synthetic(
    name: &'static str,
    days: usize,
    seed: u64,
    mut f: impl FnMut(usize, &mut Rng) -> Hour,
) -> Synthetic {
    let mut rng = Rng(seed);
    let hours: Vec<Hour> = (0..days * 24).map(|k| f(k, &mut rng)).collect();
    let mut level = 2_000.0 * GB;
    let mut points = Vec::new();
    let mut index = vec![None; hours.len()];
    for (k, h) in hours.iter().enumerate() {
        level -= h.consumed - h.cleaned;
        if !h.unseen {
            index[k] = Some(points.len());
            points.push(SpacePoint {
                ts: k as i64 * HOUR + HOUR - 60,
                total: 4_000_000_000_000,
                avail: level as u64,
            });
        }
    }
    Synthetic {
        name,
        points,
        consumed: hours.iter().map(|h| h.consumed).collect(),
        index,
    }
}

fn lognormal(rng: &mut Rng, median: f64, sigma: f64) -> f64 {
    median * (sigma * rng.normal()).exp()
}

/// Builds by day on weekdays, little at night and at weekends, a day's
/// intensity varying (lognormal), and half of what builds write cleaned by
/// the tools themselves the same evening.
fn rhythm(k: usize, rng: &mut Rng, scale: f64) -> f64 {
    let (day, hour) = (k / 24, k % 24);
    let weekend = day % 7 >= 5;
    // The same draw for every hour of a day.
    let mut day_rng = Rng(day as u64 * 7919 + 17);
    let intensity = lognormal(&mut day_rng, 1.0, 0.6) * if weekend { 0.25 } else { 1.0 };
    let base = 0.005 * GB * rng.normal();
    let builds = if (9..19).contains(&hour) && rng.f64() < 0.5 {
        lognormal(rng, 0.8 * GB, 0.7) * intensity * scale
    } else {
        0.0
    };
    // Evening: the build tools prune about half the day's output.
    let prune = if hour == 20 {
        -2.0 * GB * intensity * scale
    } else {
        0.0
    };
    base + builds + prune
}

fn scenarios() -> Vec<Synthetic> {
    let days = 35;
    vec![
        synthetic("steady 3 GB/day", days, 1, |_, r| Hour {
            consumed: 3.0 * GB / 24.0 + 0.05 * GB * r.normal(),
            ..Hour::default()
        }),
        synthetic("daily rhythm", days, 2, |k, r| Hour {
            consumed: rhythm(k, r, 1.0),
            ..Hour::default()
        }),
        synthetic("idle, then bursts", days, 3, |k, r| Hour {
            consumed: if k < 20 * 24 {
                0.1 * GB / 24.0 + 0.01 * GB * r.normal()
            } else {
                rhythm(k, r, 4.0)
            },
            ..Hour::default()
        }),
        synthetic("one-off cleanup", days, 4, |k, r| Hour {
            consumed: rhythm(k, r, 0.7),
            cleaned: if k == 17 * 24 + 15 { 30.0 * GB } else { 0.0 },
            ..Hour::default()
        }),
        synthetic("weekly cleanup", days, 5, |k, r| Hour {
            consumed: 3.0 * GB / 24.0 + 0.05 * GB * r.normal()
                - if k % (7 * 24) == 7 * 24 - 1 {
                    18.0 * GB
                } else {
                    0.0
                },
            ..Hour::default()
        }),
        synthetic("asleep at night, a 3-day gap", days, 6, |k, r| {
            let asleep = (1..8).contains(&(k % 24));
            let off = (14 * 24..17 * 24).contains(&k);
            Hour {
                consumed: if asleep { 0.0 } else { rhythm(k, r, 1.0) },
                unseen: asleep || off,
                ..Hour::default()
            }
        }),
        {
            // Like the author's disk: the net rate switches every few
            // days, hours swing by gigabytes both ways, and now and then a
            // big cleanup by hand.
            let mut regime = 0.0;
            synthetic("volatile, switching", days, 7, move |k, r| {
                if k % (24 * (2 + (k / 24) % 4)) == 0 {
                    regime = [-4.0, 2.0, 8.0, 20.0][(r.f64() * 4.0) as usize] * GB / 24.0;
                }
                let swing = if r.f64() < 0.3 {
                    lognormal(r, 3.0 * GB, 0.5) * if r.f64() < 0.5 { 1.0 } else { -0.9 }
                } else {
                    0.3 * GB * r.normal()
                };
                Hour {
                    consumed: regime + swing,
                    cleaned: if k % (11 * 24) == 11 * 24 - 5 {
                        40.0 * GB
                    } else {
                        0.0
                    },
                    ..Hour::default()
                }
            })
        },
    ]
}

/// (point, 10th, 90th percentile) of what is consumed over the next `hours`.
type Prediction = (f64, f64, f64);

fn old_method(pts: &[SpacePoint], p: &Params, hours: usize) -> Option<Prediction> {
    let last = pts.last()?;
    let (r, _) = WINDOWS
        .iter()
        .find_map(|&w| window_rate(pts, &[], last.ts - w, w, p))?;
    let k = hours as f64 / 24.0;
    Some((r.per_day * k, r.low * k, r.high * k))
}

fn new_method(pts: &[SpacePoint], p: &Params, hours: usize) -> Option<Prediction> {
    let h = Hourly::new(pts, p)?;
    let pool = Pool::new(&h, p, None, p.min_sim_hours)?;
    let out = pool.run(p.paths, hours, None, sim::seed(&h, 0.0, pts.last()?.ts));
    let c = sim::sorted(out.consumed);
    Some((
        sim::quantile(&c, 0.5),
        sim::quantile(&c, 0.1),
        sim::quantile(&c, 0.9),
    ))
}

/// The last 24 hours again, as a baseline.
fn persistence(pts: &[SpacePoint], p: &Params, hours: usize) -> Option<Prediction> {
    let v = Hourly::new(pts, p)?.mean_over(24, 0.75)? * hours as f64;
    Some((v, v, v))
}

#[derive(Debug, Default, Clone, Copy)]
struct Score {
    n: usize,
    mae: f64,
    cover: f64,
    width: f64,
    interval: f64,
}

impl Score {
    fn of(results: &[(Prediction, f64)]) -> Score {
        let n = results.len().max(1) as f64;
        let mut s = Score {
            n: results.len(),
            ..Score::default()
        };
        for &((mid, lo, hi), y) in results {
            s.mae += (mid - y).abs() / n;
            s.cover += f64::from(u8::from(lo <= y && y <= hi)) / n;
            s.width += (hi - lo) / n;
            s.interval += ((hi - lo) + 10.0 * ((lo - y).max(0.0) + (y - hi).max(0.0))) / n;
        }
        s
    }

    fn row(&self, label: &str) -> String {
        format!(
            "{label:<34} n={:<4} MAE {:>6.2} GB  cover {:>4.0}%  width {:>6.2} GB  IS {:>6.2}",
            self.n,
            self.mae / GB,
            self.cover * 100.0,
            self.width / GB,
            self.interval / GB
        )
    }
}

type Method = fn(&[SpacePoint], &Params, usize) -> Option<Prediction>;

/// Scores `method` from each origin of `h`, skipping origins every
/// `stride`.
fn score(h: &History, p: &Params, hours: usize, stride: usize, method: Method) -> Score {
    let results: Vec<(Prediction, f64)> = h
        .origins
        .iter()
        .step_by(stride.max(1))
        .filter_map(|&(i, truth)| Some((method(&h.points[..=i], p, hours)?, truth)))
        .collect();
    Score::of(&results)
}

fn mean(scores: &[Score]) -> Score {
    let n = scores.len() as f64;
    let mut m = Score::default();
    for s in scores {
        m.n += s.n;
        m.mae += s.mae / n;
        m.cover += s.cover / n;
        m.width += s.width / n;
        m.interval += s.interval / n;
    }
    m
}

#[test]
fn the_simulation_beats_the_trend_on_synthetic_histories() {
    let p = Params {
        paths: 300,
        ..Params::default()
    };
    // Every 13th hour (every 29th three days ahead) keeps it quick in a
    // debug build; the ignored tests below score every third.
    let synth = scenarios();
    for (hours, stride) in [(24usize, 13), (72, 29)] {
        let mut old = Vec::new();
        let mut new = Vec::new();
        eprintln!("next {hours} h:");
        for h in synth.iter().map(|s| s.ahead(hours)) {
            let (o, n) = (
                score(&h, &p, hours, stride, old_method),
                score(&h, &p, hours, stride, new_method),
            );
            eprintln!("{}\n  {}\n  {}", h.name, o.row("old"), n.row("new"));
            old.push(o);
            new.push(n);
        }
        let (o, n) = (mean(&old), mean(&new));
        eprintln!("{}\n{}", o.row("old, mean"), n.row("new, mean"));
        assert!(n.interval < o.interval, "{n:?} vs {o:?}");
        assert!(n.mae < o.mae, "{n:?} vs {o:?}");
        assert!(n.cover > o.cover - 0.03 && n.cover > 0.65, "{n:?}");
    }
}

/// The grid the defaults were chosen from.
#[test]
#[ignore = "slow: a tuning grid; run with --release --ignored --nocapture"]
fn tuning_grid() {
    let hs: Vec<History> = scenarios().iter().map(|s| s.ahead(24)).collect();
    let base = Params {
        paths: 400,
        ..Params::default()
    };
    let old: Vec<Score> = hs
        .iter()
        .map(|h| score(h, &base, 24, 3, old_method))
        .collect();
    let per: Vec<Score> = hs
        .iter()
        .map(|h| score(h, &base, 24, 3, persistence))
        .collect();
    println!("{}", mean(&old).row("old (Theil–Sen)"));
    println!("{}", mean(&per).row("persistence (last 24 h)"));
    let mut rows = Vec::new();
    for half_life_days in [1.0, 2.0, 3.0, 5.0, 1e6] {
        for block_hours in [1, 3, 6, 24] {
            for phase_jitter in [0, 1, 2] {
                let p = Params {
                    half_life_days,
                    block_hours,
                    phase_jitter,
                    ..base.clone()
                };
                let s: Vec<Score> = hs.iter().map(|h| score(h, &p, 24, 3, new_method)).collect();
                let m = mean(&s);
                let label =
                    format!("hl {half_life_days} d, block {block_hours} h, ±{phase_jitter} h");
                println!("{}", m.row(&label));
                rows.push((m.interval, label));
            }
        }
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    println!("best by interval score:");
    for (is, label) in rows.iter().take(8) {
        println!("  {:>6.2}  {label}", is / GB);
    }
    for (factor, recurring) in [
        (5.0, 3),
        (10.0, 3),
        (20.0, 3),
        (10.0, 2),
        (10.0, 5),
        (10.0, 99),
    ] {
        let p = Params {
            step_noise_factor: factor,
            recurring_steps: recurring,
            ..base.clone()
        };
        let s: Vec<Score> = hs.iter().map(|h| score(h, &p, 24, 3, new_method)).collect();
        println!(
            "{}",
            mean(&s).row(&format!("steps: {factor}× noise, recur at {recurring}"))
        );
    }
}

/// A real history, read-only: `MDIF_BACKTEST_DB` (a copy, please) and
/// optionally `MDIF_BACKTEST_VOL`.
#[test]
#[ignore = "needs MDIF_BACKTEST_DB"]
fn real_history() {
    let Some(path) = std::env::var_os("MDIF_BACKTEST_DB") else {
        eprintln!("MDIF_BACKTEST_DB not set");
        return;
    };
    let db = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    let vol: String = match std::env::var("MDIF_BACKTEST_VOL") {
        Ok(v) => v,
        Err(_) => db
            .query_row(
                "SELECT vol FROM space GROUP BY vol ORDER BY COUNT(*) DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap(),
    };
    let mut st = db
        .prepare("SELECT ts, total, avail FROM space WHERE vol = ?1 ORDER BY ts")
        .unwrap();
    let points: Vec<SpacePoint> = st
        .query_map([&vol], |r| {
            Ok(SpacePoint {
                ts: r.get(0)?,
                total: r.get::<_, i64>(1)? as u64,
                avail: r.get::<_, i64>(2)? as u64,
            })
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let p = Params::default();
    let all = Hourly::new(&points, &p).unwrap();
    println!(
        "{vol}: {} samples over {:.1} days; steps {:?} ({}), threshold {:.1} GB",
        points.len(),
        (points[points.len() - 1].ts - points[0].ts) as f64 / DAY as f64,
        all.steps,
        if all.removed {
            "one-off, taken out"
        } else {
            "recurring, kept"
        },
        all.threshold / GB
    );
    for hours in [24usize, 6, 1] {
        let span = hours as i64 * HOUR;
        // Origins: every sample with a day of history before it and a
        // sample `hours` later (give or take 15 minutes).
        let mut raw = Vec::new();
        let mut adjusted = Vec::new();
        for (i, o) in points.iter().enumerate() {
            if o.ts - points[0].ts < DAY {
                continue;
            }
            let j = points.partition_point(|q| q.ts <= o.ts + span);
            let Some(end) = j.checked_sub(1).map(|j| points[j]) else {
                continue;
            };
            if j <= i + 1 || (o.ts + span - end.ts) > HOUR / 4 {
                continue;
            }
            let consumed = o.avail as f64 - end.avail as f64;
            let cleaned: f64 = points[i..j]
                .windows(2)
                .map(|w| w[1].avail as f64 - w[0].avail as f64)
                .filter(|&d| all.removed && d >= all.threshold)
                .sum();
            raw.push((i, consumed));
            adjusted.push((i, consumed + cleaned));
        }
        for (what, origins) in [("raw", raw), ("one-off steps out", adjusted)] {
            let h = History {
                name: "real",
                points: points.clone(),
                origins,
            };
            println!("next {hours} h, what happened {what}:");
            for (label, m) in [
                ("  old (Theil–Sen)", old_method as Method),
                ("  new (simulation)", new_method),
                ("  persistence", persistence),
            ] {
                println!("{}", score(&h, &p, hours, 1, m).row(label));
            }
        }
    }
}

/// The half-life, scenario by scenario, at 24 and 72 hours ahead.
#[test]
#[ignore = "slow: run with --release --ignored --nocapture"]
fn half_life_by_scenario() {
    let synth = scenarios();
    for hours in [24usize, 72] {
        let hs: Vec<History> = synth.iter().map(|s| s.ahead(hours)).collect();
        println!("next {hours} h: interval score (GB), coverage");
        let base = Params {
            paths: 400,
            ..Params::default()
        };
        let cell = |s: Score| format!("{:>6.1} {:>3.0}%", s.interval / GB, s.cover * 100.0);
        let mut line = format!("{:<30}", "old (Theil–Sen)");
        for h in &hs {
            line += &cell(score(h, &base, hours, 3, old_method));
        }
        println!("{line}");
        for half_life_days in [1.0, 2.0, 3.0, 5.0, 1e6] {
            for block_hours in [3, 6] {
                let p = Params {
                    half_life_days,
                    block_hours,
                    phase_jitter: 1,
                    ..base.clone()
                };
                let mut line = format!(
                    "{:<30}",
                    format!("hl {half_life_days} d, block {block_hours} h")
                );
                let mut all = Vec::new();
                for h in &hs {
                    let s = score(h, &p, hours, 3, new_method);
                    line += &cell(s);
                    all.push(s);
                }
                let m = mean(&all);
                line += &format!("  | mean {} MAE {:.2}", cell(m), m.mae / GB);
                println!("{line}");
            }
        }
    }
}

/// Block length at 24 and 72 hours ahead.
#[test]
#[ignore = "slow: run with --release --ignored --nocapture"]
fn block_length() {
    let synth = scenarios();
    for hours in [24usize, 72] {
        let hs: Vec<History> = synth.iter().map(|s| s.ahead(hours)).collect();
        let base = Params {
            paths: 400,
            ..Params::default()
        };
        let old: Vec<Score> = hs
            .iter()
            .map(|h| score(h, &base, hours, 3, old_method))
            .collect();
        println!("next {hours} h\n{}", mean(&old).row("old (Theil–Sen)"));
        for half_life_days in [3.0, 1e6] {
            for block_hours in [3, 6, 12, 24] {
                for phase_jitter in [0, 1] {
                    let p = Params {
                        half_life_days,
                        block_hours,
                        phase_jitter,
                        ..base.clone()
                    };
                    let s: Vec<Score> = hs
                        .iter()
                        .map(|h| score(h, &p, hours, 3, new_method))
                        .collect();
                    println!(
                        "{}",
                        mean(&s).row(&format!(
                            "hl {half_life_days}, block {block_hours} ±{phase_jitter}"
                        ))
                    );
                }
            }
        }
    }
}

/// Half-life × concentration, at 24 and 72 hours ahead.
#[test]
#[ignore = "slow: run with --release --ignored --nocapture"]
fn half_life_and_concentration() {
    let synth = scenarios();
    let by: Vec<Vec<History>> = [24usize, 72]
        .iter()
        .map(|&h| synth.iter().map(|s| s.ahead(h)).collect())
        .collect();
    let base = Params {
        paths: 400,
        block_hours: 6,
        phase_jitter: 1,
        ..Params::default()
    };
    let cell = |s: Score| format!("{:>6.1} {:>3.0}%", s.interval / GB, s.cover * 100.0);
    for (hs, hours) in by.iter().zip([24usize, 72]) {
        let mut line = format!("{:<22}", format!("{hours} h old"));
        let old: Vec<Score> = hs
            .iter()
            .map(|h| score(h, &base, hours, 3, old_method))
            .collect();
        for s in &old {
            line += &cell(*s);
        }
        let m = mean(&old);
        println!("{line}  | {} MAE {:.2}", cell(m), m.mae / GB);
    }
    let mut rows = Vec::new();
    for half_life_days in [2.0, 3.0, 5.0, 7.0, 1e6] {
        for concentration in [1.0, 0.5, 0.25, 0.1] {
            let p = Params {
                half_life_days,
                concentration,
                ..base.clone()
            };
            let mut total = 0.0;
            for (hs, hours) in by.iter().zip([24usize, 72]) {
                let s: Vec<Score> = hs
                    .iter()
                    .map(|h| score(h, &p, hours, 3, new_method))
                    .collect();
                let mut line = format!(
                    "{:<22}",
                    format!("{hours} h hl {half_life_days} c {concentration}")
                );
                for x in &s {
                    line += &cell(*x);
                }
                let m = mean(&s);
                // Each horizon relative to the old method's score.
                total += m.interval
                    / mean(
                        &hs.iter()
                            .map(|h| score(h, &base, hours, 3, old_method))
                            .collect::<Vec<_>>(),
                    )
                    .interval;
                println!("{line}  | {} MAE {:.2}", cell(m), m.mae / GB);
            }
            rows.push((
                total / 2.0,
                format!("hl {half_life_days} c {concentration}"),
            ));
        }
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    println!("interval score relative to the old method, both horizons:");
    for (r, label) in rows {
        println!("  {r:.3}  {label}");
    }
}

/// What a forecast costs, at its most expensive: two weeks of history
/// (5-minute rows for the last day, hourly before, as the store keeps
/// them), every path running to the horizon, or an alert's leave-one-day-out
/// runs.
#[test]
#[ignore = "timing: run with --release --ignored --nocapture"]
fn forecast_cost() {
    let history = |gb_per_day: f64| -> Vec<SpacePoint> {
        let end = 31 * DAY;
        (0..31 * 24)
            .map(|k| k * HOUR)
            .chain((end - DAY..=end).step_by(300).skip(1))
            .filter(|&ts| ts < end - DAY || ts % 300 == 0)
            .map(|ts| SpacePoint {
                ts,
                total: 500_000_000_000,
                avail: (400.0 * GB - gb_per_day * GB * ts as f64 / DAY as f64
                    + 2.0 * GB * (ts as f64 / 7000.0).sin()) as u64,
            })
            .collect()
    };
    for (label, gb) in [
        ("not filling (to the horizon)", 0.5),
        ("filling in weeks", 9.0),
        ("alerting (leave-one-day-out)", 11.0),
    ] {
        let pts = history(gb);
        let t = std::time::Instant::now();
        let runs = 20;
        let mut f = None;
        for _ in 0..runs {
            f = forecast(&pts, &Params::default());
        }
        let f = f.unwrap();
        println!(
            "{label}: {:.1} ms a forecast ({} points), full in {:?}, alert {:?}",
            t.elapsed().as_secs_f64() * 1000.0 / runs as f64,
            pts.len(),
            f.full_in.map(|x| x.days),
            alert(Some(&f), f.avail, 500_000_000_000)
        );
    }
}
