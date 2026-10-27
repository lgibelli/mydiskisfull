//! `mdif watch` (the sampler) and `mdif forecast` (what its history says).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Local, TimeZone};
use mdif_core::forecast::{self, DAY, Forecast, FullIn, HOUR, Method, Params, Rate};
use mdif_core::owners::Rules;
use mdif_core::store::{Cleanup, Store};
use mdif_core::suggest;
use mdif_core::text::printable;
use mdif_core::units::bytes;
use mdif_core::watch::{self, Cost, Watcher};
use serde_json::{Value, json};

/// The watcher's own budget, from the plan.
const BUDGET_CPU_PER_HOUR: Duration = Duration::from_secs(1);
const BUDGET_WRITES_PER_HOUR: u64 = 1_000_000;

pub fn default_db() -> PathBuf {
    data_home().join("Library/Application Support/MyDiskIsFull/history.sqlite3")
}

/// Where the agent's errors go.
pub fn log_path() -> PathBuf {
    data_home().join("Library/Logs/MyDiskIsFull/watch.log")
}

/// How long after it starts the watcher first looks at the tidy-ups.
const TIDY_DELAY: i64 = 600;

/// The log is started afresh, keeping one old copy, once it passes this.
const LOG_MAX: u64 = 1_000_000;

/// Send standard error to the log: the LaunchAgent inside the app can't name
/// a path in the user's home folder in its plist.
fn open_log() -> Result<()> {
    let path = log_path();
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX) {
        std::fs::rename(&path, path.with_extension("log.old"))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    mdif_macos::sys::redirect_stderr(&file).context("redirecting errors to the log")?;
    Ok(())
}

/// When this command started: `main` sets it first thing.
static STARTED: OnceLock<i64> = OnceLock::new();

pub(crate) fn mark_started() {
    STARTED.get_or_init(watch::unix_now);
}

/// Record a clean-up done through mdif, from `start` to now, so the forecast
/// takes out what came back around it, however little (see
/// mdif_core::forecast::forecast_with). Never fatal: the clean-up is done.
pub(crate) fn record_cleanup(store: &Store, start: i64, freed: u64) {
    let c = Cleanup {
        start,
        end: watch::unix_now(),
        freed,
    };
    if let Err(e) = store.add_cleanup(&c) {
        eprintln!("The clean-up isn't in the history, so the forecast may misread it: {e:#}");
    }
}

/// [`record_cleanup`] for `mdif clean`: since the command started, in the
/// history at `db` if there is one.
pub(crate) fn record_clean(db: &Path, freed: u64) {
    if !db.exists() {
        return;
    }
    match Store::open(db) {
        Ok(store) => record_cleanup(&store, *STARTED.get_or_init(watch::unix_now), freed),
        Err(e) => eprintln!("The clean-up isn't in the history: {e:#}"),
    }
}

/// History database, WAL and shared memory together.
pub(crate) fn history_bytes(db: &Path) -> u64 {
    ["", "-wal", "-shm"]
        .iter()
        .filter_map(|s| std::fs::metadata(format!("{}{s}", db.display())).ok())
        .map(|m| m.len())
        .sum()
}

pub struct WatchOptions {
    pub interval: Duration,
    pub owners_every: u64,
    pub once: bool,
    pub quiet: bool,
    pub log: bool,
    pub attached: bool,
}

pub fn watch(db: &Path, volume: &Path, o: WatchOptions) -> Result<()> {
    let WatchOptions {
        interval,
        owners_every,
        once,
        quiet,
        log,
        attached,
    } = o;
    if log {
        open_log()?;
    }
    if attached {
        exit_when_stdin_closes();
    }
    let store = Store::open(db)?;
    let mut w =
        Watcher::new(store, volume)?.with_access_check(mdif_macos::sys::access_probes(&home()));
    if owners_every > 0 && !once {
        w = w.with_owners(Rules::for_home(&home()), owners_every as i64)?;
    }
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
    let home = home();
    // Tidy-ups (`mdif tidy`) are looked at once an hour, after a sample, and
    // run when one that's on hasn't for a day; not in the first minutes,
    // while what starts at login settles.
    let tidy_store = Store::open(db)
        .inspect_err(|e| eprintln!("No tidy-ups: {e:#}"))
        .ok();
    let mut tidy_next = watch::unix_now() + TIDY_DELAY;
    watch::run(
        &mut w,
        interval,
        || false,
        |s| match s {
            Ok(s) => {
                if !quiet {
                    print_sample(s);
                }
                // Logged even when quiet: the agent's log is where they go.
                for i in &s.opened {
                    eprintln!(
                        "{} incident: {} Fix: {}",
                        local(s.point.ts).format("%F %H:%M"),
                        i.sentence(Some(&home)),
                        crate::tweaks_cmd::fix_line(&suggest::for_incident(
                            i,
                            &home,
                            &crate::keep_cmd::load_or_warn(db)
                        ))
                    );
                }
                if let Some(store) = &tidy_store
                    && s.point.ts >= tidy_next
                {
                    tidy_next = s.point.ts + HOUR;
                    crate::tidy_cmd::run_due(store, db, s.point.ts);
                }
            }
            Err(e) => eprintln!("{} sample failed: {e:#}", local_now().format("%F %T")),
        },
    );
    Ok(())
}

/// The App Store edition's app runs the watcher while it's open, with a pipe
/// for standard input: when the app quits, or crashes, the pipe closes and
/// the watcher goes too, even in the middle of its sleep.
fn exit_when_stdin_closes() {
    std::thread::spawn(|| {
        let mut sink = [0u8; 64];
        let mut stdin = std::io::stdin().lock();
        while std::io::Read::read(&mut stdin, &mut sink).is_ok_and(|n| n > 0) {}
        std::process::exit(0);
    });
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
    if let Some(p) = s.period {
        println!(
            "      folders: {} changes, {} folders re-listed ({} entries), {} remembered{}{}",
            p.events,
            p.listed,
            p.entries,
            p.tracked,
            if p.skipped > 0 {
                format!(", {} left alone (they need Full Disk Access)", p.skipped)
            } else {
                String::new()
            },
            if p.gaps > 0 {
                ", history incomplete"
            } else {
                ""
            }
        );
        for (dir, n) in &s.biggest {
            println!("      {n:>9} entries  {}", printable(&tilde(dir)));
        }
        for (owner, d) in s.growth.iter().take(5) {
            let sign = if *d < 0 { "-" } else { "+" };
            println!(
                "      {sign}{:>9}  {}",
                bytes(d.unsigned_abs()),
                printable(&tilde(owner))
            );
        }
    }
}

/// The home folder whose folders are watched and shown: `$HOME`, except in
/// the App Sandbox, where `$HOME` is the app's container and this is the
/// account's own.
pub(crate) fn home() -> PathBuf {
    if sandboxed()
        && let Some(home) = mdif_macos::sys::account_home()
    {
        return home;
    }
    data_home()
}

/// Where the history and the log are kept: `$HOME`, which in the App Sandbox
/// is the app's container.
fn data_home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// Running in the App Sandbox (the App Store edition), which names the
/// container it gives the process.
pub(crate) fn sandboxed() -> bool {
    std::env::var_os("APP_SANDBOX_CONTAINER_ID").is_some()
}

/// `/Users/me/x` as `~/x`.
pub(crate) fn tilde(path: &str) -> String {
    let home = home();
    match Path::new(path).strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.to_string(),
    }
}

pub fn forecast(db: &Path, volume: &Path, json_out: bool, series: bool) -> Result<()> {
    if !db.exists() && json_out {
        // Nothing yet, but still JSON for whoever reads it.
        let out = json!({
            "volume": volume,
            "samples": 0,
            "forecast": null,
            "writes_24h": [],
            "growth_24h": [],
            "watcher_cost": null,
            "last_period": null,
            "series": [],
            "writes_daily": [],
            "strata": [],
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
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
    let cleanups = store.cleanups_since(pts.first().map_or(now, |p| p.ts))?;
    let f = forecast::forecast_with(
        &pts,
        &crate::status_cmd::facts(volume, &pts, &cleanups, now),
        &Params::default(),
    );
    let writers = store.writes_since(now - DAY)?;
    let cost = watch::recorded_cost(&store)?;
    let last_period = watch::recorded_period(&store)?;
    let interval = sampling_interval(&pts);

    if json_out {
        let mut out = json!({
            "volume": key,
            "samples": pts.len(),
            "forecast": f,
            "writes_24h": writers.iter().map(|(n, b)| json!({"name": n, "bytes": b})).collect::<Vec<_>>(),
            "growth_24h": store.growth_since(now - DAY)?.iter().map(|(o, b)| json!({"owner": o, "bytes": b})).collect::<Vec<_>>(),
            "growth_3d": growth_json(&store, now - 3 * DAY)?,
            "growth_14d": growth_json(&store, now - 14 * DAY)?,
            "watcher_cost": cost.map(|c| cost_json(&c, interval)),
            "last_period": last_period,
        });
        if series {
            // [unix seconds, bytes free], at the resolution the store keeps.
            out["series"] = pts.iter().map(|p| json!([p.ts, p.avail])).collect();
            out["writes_daily"] = store
                .writes_daily(now - 400 * DAY, &mdif_macos::sys::utc_offset, 8)?
                .iter()
                .map(|d| {
                    json!({
                        "day": d.day,
                        "bytes": d.bytes,
                        "top": d.top.iter().map(|(n, b)| json!({"name": n, "bytes": b})).collect::<Vec<_>>(),
                    })
                })
                .collect();
            // What the disk was made of at each whole measurement.
            out["strata"] = store
                .strata_since(now - 400 * DAY)?
                .iter()
                .map(|(ts, layers)| {
                    let l: serde_json::Map<String, serde_json::Value> =
                        layers.iter().map(|(k, b)| (k.clone(), json!(b))).collect();
                    json!({"ts": ts, "layers": l})
                })
                .collect();
        }
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    let Some(f) = f else {
        println!("No samples for {key} yet. Is the watcher running? (`mdif agent status`)");
        return Ok(());
    };
    let keep = crate::keep_cmd::load_or_warn(db);
    let open: Vec<_> = store
        .incidents_since(now)?
        .into_iter()
        .filter(|e| e.closed.is_none())
        .collect();
    for e in &open {
        println!("Runaway: {}", e.incident.sentence(Some(&home())));
        let fix = suggest::for_incident(&e.incident, &home(), &keep);
        println!("  Fix: {}", crate::tweaks_cmd::fix_line(&fix));
    }
    if !open.is_empty() {
        println!();
    }
    print_forecast(&key, &f, pts.len(), pts.first().map(|p| p.ts));
    print_growth(&store.growth_since(now - DAY)?);
    print_writers(&writers);
    if let Some(c) = cost {
        print_cost(&c, interval);
    }
    if let Some(p) = last_period {
        print_last_period(&p);
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
    let range = format!("plausibly {} to {} a day", signed(r.low), signed(r.high));
    if r.per_day <= 0.0 {
        println!(
            "Not filling: free space is stable or growing ({} a day, {}; {range}).",
            bytes(r.per_day.abs() as u64),
            basis(f, &r)
        );
    } else {
        println!(
            "Filling at about {} a day ({}; {range}).",
            bytes(r.per_day as u64),
            basis(f, &r)
        );
    }
    match (f.full_in, f.method) {
        (Some(full), _) => println!("{}", full_sentence(f.at, &full)),
        (None, Method::Simulation) if r.per_day > 0.0 => println!(
            "Not full within {} days in most of the simulated futures.",
            Params::default().horizon_days
        ),
        _ => {}
    }
    if let Some(by) = f.full_by {
        println!(
            "Chance of being full: {} within a day, {} within 3 days, {} within 14 days.",
            percent(by.d1),
            percent(by.d3),
            percent(by.d14)
        );
    }
    println!("{}", paces_sentence(&f.paces));
    if f.method == Method::Simulation
        && let Some(t) = f.trend
    {
        println!(
            "For comparison, the {} trend (Theil–Sen): {} a day.",
            window(&t),
            signed(t.per_day)
        );
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
            "taken out of the forecast (one-off)"
        } else {
            "kept in the forecast (they recur)"
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
    if !f.arrivals.is_empty() {
        let list: Vec<String> = f
            .arrivals
            .iter()
            .rev()
            .take(3)
            .map(|a| {
                format!(
                    "-{} on {}",
                    bytes(a.bytes),
                    local(a.ts).format("%b %-d %H:%M")
                )
            })
            .collect();
        println!(
            "Space taken in one go, taken out of the forecast (one-off): {}.",
            list.join(", ")
        );
    }
    if f.purgeable > 0 {
        println!(
            "The time to full counts {} that macOS frees by itself when it's needed (purgeable).",
            bytes(f.purgeable)
        );
    }
}

/// What grew the most since `since` and what shrank the most (25 and 10
/// owners at most), for the app's lists over a longer window.
fn growth_json(store: &Store, since: i64) -> Result<Vec<Value>> {
    let all = store.growth_since(since)?;
    let grew = all.iter().take_while(|(_, b)| *b > 0).take(25);
    let shrank = all.iter().rev().take_while(|(_, b)| *b < 0).take(10);
    Ok(grew
        .chain(shrank)
        .map(|(o, b)| json!({"owner": o, "bytes": b}))
        .collect())
}

/// Where the forecast rate comes from.
pub(crate) fn basis(f: &Forecast, r: &Rate) -> String {
    match f.method {
        Method::Simulation => format!(
            "{} futures drawn from the last {}",
            Params::default().paths,
            duration(r.window)
        ),
        Method::Trend => format!("the {} trend, until there's a day of history", window(r)),
    }
}

pub(crate) fn percent(p: f64) -> String {
    format!("{:.0}%", p * 100.0)
}

/// Bytes a day, with a minus sign when space comes back.
fn signed(per_day: f64) -> String {
    if per_day < 0.0 {
        format!("-{}", bytes(per_day.abs() as u64))
    } else {
        bytes(per_day as u64)
    }
}

pub(crate) fn paces_sentence(p: &forecast::Paces) -> String {
    let one = |v: Option<f64>, over: &str| match v {
        Some(v) => format!("{} a day over the last {over}", signed(v)),
        None => format!("not enough samples for the last {over}"),
    };
    format!(
        "Pace: {}; {}; {}.",
        one(p.hour, "hour"),
        one(p.day, "24 hours"),
        one(p.week, "7 days")
    )
}

/// "Full in about 6 days, around Oct 27 (likely between Oct 23 and Nov 12)."
fn full_sentence(at: i64, full: &FullIn) -> String {
    format!(
        "Full in about {}, around {} ({}).",
        days(full.days),
        date(at, full.days),
        full_range(at, full)
    )
}

pub(crate) fn full_range(at: i64, full: &FullIn) -> String {
    match full.latest {
        Some(latest) => format!(
            "likely between {} and {}",
            date(at, full.earliest),
            date(at, latest)
        ),
        None => format!(
            "as early as {}, or not for a long time",
            date(at, full.earliest)
        ),
    }
}

fn print_growth(growth: &[(String, i64)]) {
    let grew: Vec<_> = growth.iter().filter(|(_, b)| *b > 0).take(6).collect();
    if grew.is_empty() {
        return;
    }
    println!();
    println!("Folders that grew the most in the last 24 hours (`mdif owners` for more):");
    for (owner, b) in grew {
        println!("  {:>9}  {}", bytes(*b as u64), printable(&tilde(owner)));
    }
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

pub(crate) fn cost_json(c: &Cost, interval: i64) -> Value {
    let (cpu, written) = c.per_sample();
    let (cpu_h, written_h) = c.per_hour(interval);
    json!({
        "samples": c.samples,
        "interval_secs": interval,
        "cpu_us_per_sample": cpu.as_micros() as u64,
        "written_per_sample": written,
        "cpu_us_per_hour": cpu_h.as_micros() as u64,
        "written_per_hour": written_h,
        "owners_every_secs": c.every,
        "periods": c.periods,
        "cpu_us_per_period": c.per_period().map(|p| p.as_micros() as u64),
        "catch_up_cpu_us": c.catch_up.as_micros() as u64,
    })
}

fn print_cost(c: &Cost, interval: i64) {
    let (cpu, written) = c.per_sample();
    let (cpu_h, written_h) = c.per_hour(interval);
    let ok = cpu_h <= BUDGET_CPU_PER_HOUR && written_h <= BUDGET_WRITES_PER_HOUR;
    let ms = |d: Duration| d.as_millis();
    let updates = match (c.every, c.per_period()) {
        (0, _) | (_, None) => String::new(),
        (every, Some(p)) if c.periods == 0 => format!(
            ", and {} ms for the first update of the folders (one every {}; the next ones \
             cost less, but none has run yet)",
            ms(p),
            duration(every)
        ),
        (every, Some(p)) => format!(
            ", and {} ms for each update of the folders (one every {})",
            ms(p),
            duration(every)
        ),
    };
    println!();
    println!(
        "Watcher cost: {:.1} ms CPU and {} written per sample{updates}. At one sample every {}, \
         that is {} ms and {} per hour ({}: 1 s and 1 MB).",
        cpu.as_secs_f64() * 1000.0,
        bytes(written),
        duration(interval),
        ms(cpu_h),
        bytes(written_h),
        if ok { "within budget" } else { "OVER BUDGET" }
    );
    if c.periods > 0 && !c.catch_up.is_zero() {
        println!(
            "The first update after it started took {} ms: it caught up on what changed while \
             the watcher wasn't running.",
            ms(c.catch_up)
        );
    }
}

fn print_last_period(p: &watch::LastPeriod) {
    let biggest: Vec<String> = p
        .biggest
        .iter()
        .map(|(d, n)| format!("{} ({n})", printable(&tilde(d))))
        .collect();
    println!(
        "The last update of the folders ({}) listed {} folders, {} entries in all, in {} ms{}.",
        local(p.at).format("%H:%M"),
        p.listed,
        p.entries,
        p.cpu_us / 1000,
        if biggest.is_empty() {
            String::new()
        } else {
            format!("; the biggest: {}", biggest.join(", "))
        }
    );
    if p.skipped > 0 {
        println!(
            "It left {} folders alone: they need Full Disk Access, and asking for it in the \
             background would wait on a dialog.",
            p.skipped
        );
    }
}

/// Median gap between the most recent samples; the agent's default if
/// there aren't enough.
pub(crate) fn sampling_interval(pts: &[mdif_core::store::SpacePoint]) -> i64 {
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

pub(crate) fn window(r: &Rate) -> String {
    if r.window >= DAY {
        format!("{}-day", r.window / DAY)
    } else {
        format!("{}-hour", r.window / HOUR)
    }
}

pub(crate) fn days(d: f64) -> String {
    let (n, unit) = if d < 1.0 {
        ((d * 24.0).round().max(1.0), "hour")
    } else if d < 100.0 {
        (d.round(), "day")
    } else {
        ((d / 30.4).round(), "month")
    };
    if n == 1.0 {
        format!("1 {unit}")
    } else {
        format!("{n} {unit}s")
    }
}

pub(crate) fn date(at: i64, in_days: f64) -> String {
    let when = local(at + (in_days * DAY as f64) as i64);
    if in_days < 2.0 {
        when.format("%a %H:%M").to_string()
    } else if in_days > 300.0 {
        when.format("%b %Y").to_string()
    } else {
        when.format("%b %-d").to_string()
    }
}

pub(crate) fn duration(secs: i64) -> String {
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

pub(crate) fn local(ts: i64) -> DateTime<Local> {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .unwrap_or_else(local_now)
}

fn local_now() -> DateTime<Local> {
    Local::now()
}
