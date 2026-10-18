//! `mdif watch` (the sampler) and `mdif forecast` (what its history says).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Local, TimeZone};
use mdif_core::forecast::{self, DAY, Forecast, FullIn, HOUR, Params, Rate};
use mdif_core::store::Store;
use mdif_core::text::printable;
use mdif_core::units::bytes;
use mdif_core::watch::{self, Cost, Watcher};
use serde_json::json;

/// The watcher's own budget, from the plan.
const BUDGET_CPU_PER_HOUR: Duration = Duration::from_secs(1);
const BUDGET_WRITES_PER_HOUR: u64 = 1_000_000;

pub fn default_db() -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    home.join("Library/Application Support/MyDiskIsFull/history.sqlite3")
}

pub fn watch(db: &Path, volume: &Path, interval: Duration, once: bool, quiet: bool) -> Result<()> {
    let store = Store::open(db)?;
    let mut w = Watcher::new(store, volume)?;
    if once {
        print_sample(&w.sample(watch::unix_now())?);
        return Ok(());
    }
    if !quiet {
        eprintln!(
            "watching {} every {} s, history in {}",
            w.volume_key(),
            interval.as_secs(),
            db.display()
        );
    }
    watch::run(
        &mut w,
        interval,
        || false,
        |s| match s {
            Ok(s) if !quiet => print_sample(s),
            Ok(_) => {}
            Err(e) => eprintln!("{} sample failed: {e:#}", local_now().format("%F %T")),
        },
    );
    Ok(())
}

fn print_sample(s: &watch::Sample) {
    let top = s
        .writes
        .first()
        .map(|(n, b)| format!("   top writer: {} ({})", printable(n), bytes(*b)))
        .unwrap_or_default();
    println!(
        "{}  free {}{}",
        local(s.point.ts).format("%F %H:%M"),
        bytes(s.point.avail),
        top
    );
}

pub fn forecast(db: &Path, volume: &Path, json_out: bool) -> Result<()> {
    if !db.exists() {
        println!(
            "No history yet. Start the watcher with `mdif agent install` (or run `mdif watch`)."
        );
        return Ok(());
    }
    let store = Store::open(db)?;
    let key = mdif_macos::fs::statfs(volume)
        .with_context(|| format!("statfs {}", volume.display()))?
        .mount_point;
    let now = watch::unix_now();
    let pts = store.space_since(&key, now - 400 * DAY)?;
    let f = forecast::forecast(&pts, &Params::default());
    let writers = store.writes_since(now - DAY)?;
    let cost = watch::recorded_cost(&store)?;
    let interval = sampling_interval(&pts);

    if json_out {
        let out = json!({
            "volume": key,
            "samples": pts.len(),
            "forecast": f,
            "writes_24h": writers.iter().map(|(n, b)| json!({"name": n, "bytes": b})).collect::<Vec<_>>(),
            "watcher_cost": cost.map(|c| {
                let (cpu, written) = c.per_sample();
                let (cpu_h, written_h) = c.per_hour(interval);
                json!({
                    "samples": c.samples,
                    "interval_secs": interval,
                    "cpu_us_per_sample": cpu.as_micros() as u64,
                    "written_per_sample": written,
                    "cpu_us_per_hour": cpu_h.as_micros() as u64,
                    "written_per_hour": written_h,
                })
            }),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    let Some(f) = f else {
        println!("No samples for {key} yet. Is the watcher running? (`mdif agent status`)");
        return Ok(());
    };
    print_forecast(&key, &f, pts.len(), pts.first().map(|p| p.ts));
    print_writers(&writers);
    if let Some(c) = cost {
        print_cost(&c, interval);
    }
    Ok(())
}

fn print_forecast(key: &str, f: &Forecast, samples: usize, since: Option<i64>) {
    println!(
        "Free now: {} on {}  ({})",
        bytes(f.avail),
        key,
        local(f.at).format("%b %-d %H:%M")
    );
    println!();
    let Some(r) = f.rate else {
        let span = since.map_or(0, |s| f.at - s);
        println!(
            "Collecting history: {} over {}. A forecast needs at least 8 samples covering 3 hours.",
            plural(samples, "sample"),
            duration(span)
        );
        return;
    };
    if r.per_day <= 0.0 {
        println!(
            "Not filling: free space is stable or growing ({} a day over {}).",
            bytes(r.per_day.abs() as u64),
            window(&r)
        );
    } else {
        println!(
            "Filling at {} a day ({} trend from {}, plausibly {}–{} a day).",
            bytes(r.per_day as u64),
            window(&r),
            plural(r.points, "sample"),
            bytes(r.low.max(0.0) as u64),
            bytes(r.high.max(0.0) as u64)
        );
        if let Some(full) = f.full_in {
            println!("{}", full_sentence(f.at, &full));
        }
    }
    if let (Some(rc), Some(soon)) = (f.recent, f.recent_full_in) {
        println!(
            "Speeding up: {} a day over the last {}. At that pace the disk is full in {} ({}).",
            bytes(rc.per_day as u64),
            window(&rc),
            days(soon.days),
            date(f.at, soon.days)
        );
    }
    if !f.steps.is_empty() {
        let how = if r.steps_removed > 0 {
            "taken out of the trend (one-off)"
        } else {
            "kept in the trend (they recur)"
        };
        let list: Vec<String> = f
            .steps
            .iter()
            .rev()
            .take(3)
            .map(|s| {
                format!(
                    "+{} on {}",
                    bytes(s.freed),
                    local(s.ts).format("%b %-d %H:%M")
                )
            })
            .collect();
        println!(
            "Space that came back in one go, {how}: {}.",
            list.join(", ")
        );
    }
}

fn full_sentence(at: i64, full: &FullIn) -> String {
    let range = match full.latest {
        Some(latest) => format!(
            "between {} and {}",
            date(at, full.earliest),
            date(at, latest)
        ),
        None => format!(
            "as early as {}, or never at the slowest plausible rate",
            date(at, full.earliest)
        ),
    };
    format!(
        "Full in about {}, around {} ({range}).",
        days(full.days),
        date(at, full.days)
    )
}

fn print_writers(writers: &[(String, u64)]) {
    if writers.is_empty() {
        return;
    }
    println!();
    println!("Most written to disk in the last 24 hours:");
    for (name, b) in writers.iter().take(8) {
        println!("  {:>9}  {}", bytes(*b), printable(name));
    }
}

fn print_cost(c: &Cost, interval: i64) {
    let (cpu, written) = c.per_sample();
    let (cpu_h, written_h) = c.per_hour(interval);
    let ok = cpu_h <= BUDGET_CPU_PER_HOUR && written_h <= BUDGET_WRITES_PER_HOUR;
    println!();
    println!(
        "Watcher cost: {:.1} ms CPU and {} written per sample. At one sample every {}, that is \
         {} ms and {} per hour ({}: 1 s and 1 MB).",
        cpu.as_secs_f64() * 1000.0,
        bytes(written),
        duration(interval),
        cpu_h.as_millis(),
        bytes(written_h),
        if ok { "within budget" } else { "OVER BUDGET" }
    );
}

/// Median gap between the most recent samples; the agent's default if
/// there aren't enough.
fn sampling_interval(pts: &[mdif_core::store::SpacePoint]) -> i64 {
    let recent = &pts[pts.len().saturating_sub(50)..];
    let mut gaps: Vec<i64> = recent
        .windows(2)
        .map(|w| w[1].ts - w[0].ts)
        .filter(|&g| g > 0)
        .collect();
    if gaps.len() < 3 {
        return 300;
    }
    gaps.sort_unstable();
    gaps[gaps.len() / 2]
}

fn window(r: &Rate) -> String {
    if r.window >= DAY {
        format!("{}-day", r.window / DAY)
    } else {
        format!("{}-hour", r.window / HOUR)
    }
}

fn days(d: f64) -> String {
    if d < 1.0 {
        format!("{} hours", (d * 24.0).round().max(1.0))
    } else if d < 100.0 {
        format!("{} days", d.round())
    } else {
        format!("{} months", (d / 30.4).round())
    }
}

fn date(at: i64, in_days: f64) -> String {
    let when = local(at + (in_days * DAY as f64) as i64);
    if in_days < 2.0 {
        when.format("%a %H:%M").to_string()
    } else if in_days > 300.0 {
        when.format("%b %Y").to_string()
    } else {
        when.format("%b %-d").to_string()
    }
}

fn duration(secs: i64) -> String {
    if secs >= 2 * DAY {
        format!("{} days", secs / DAY)
    } else if secs >= 2 * HOUR {
        format!("{} hours", secs / HOUR)
    } else if secs >= 120 {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

fn local(ts: i64) -> DateTime<Local> {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .unwrap_or_else(local_now)
}

fn local_now() -> DateTime<Local> {
    Local::now()
}
