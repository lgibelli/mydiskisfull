//! The watcher: one cheap sample every few minutes.
//!
//! A sample is one `statfs` of the watched volume plus one `proc_pid_rusage`
//! per process of the user, a few milliseconds of CPU. Write counters are
//! cumulative per process, so each sample stores what every process wrote
//! since the previous sample, summed per process name. Small writers are
//! pooled as "(others)" to keep the history bounded.
//!
//! Every half hour or so it also replays the FSEvents journal and updates how
//! much each owner folder grew ([`crate::owners`]).
//!
//! The watcher records its own CPU time and writes, so its budget ("a monitor
//! must never become the thing it hunts") is measured, not assumed.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use mdif_macos::fsevents::{self, Stream, flag};
use mdif_macos::{fs, proc};

use crate::alerts;
use crate::forecast::{DAY, HOUR};
use crate::incidents::{self, ChurnParams, GrowthParams, Incident};
use crate::map::DATA_ROOT;
use crate::owners::{Rules, Tracker};
use crate::privacy;
use crate::scan;
use crate::store::{SpacePoint, Store};

/// Writers below this per sample are pooled into [`OTHERS`].
pub const MIN_WRITER: u64 = 1_000_000;
pub const OTHERS: &str = "(others)";

pub struct Watcher {
    store: Store,
    volume: PathBuf,
    key: String,
    prev: HashMap<i32, (String, u64)>,
    primed: bool,
    last_compact: i64,
    cost: Cost,
    owners: Option<Owners>,
    detect: Detect,
    /// The thresholds the user set (`mdif alerts`), read again before each
    /// owners period, so a change applies within one without a restart.
    user: alerts::Settings,
    /// Sizes of owners measured for the growth clauses, and when.
    sizes: HashMap<String, (i64, Option<u64>)>,
    /// Bytes each process wrote since the last owners period.
    period_writes: HashMap<i32, u64>,
    /// Files that tell whether the watcher has Full Disk Access
    /// (`sys::access_probes`); checked at every sample (one `open`), so a
    /// grant is noticed within minutes.
    access_probes: Vec<PathBuf>,
    /// Free space at the last sample.
    last_avail: Option<u64>,
}

/// What counts as an incident. The defaults are the plan's clauses.
#[derive(Debug, Clone, Default)]
pub struct Detect {
    pub growth: GrowthParams,
    pub churn: ChurnParams,
}

impl Detect {
    /// With the sizes the user set instead of these (see [`alerts`]); the
    /// clauses stay.
    pub fn with_user(&self, s: &alerts::Settings) -> Detect {
        let mut d = self.clone();
        if let Some(b) = s.growth_min {
            d.growth.min_growth = b;
        }
        if let Some(b) = s.churn_per_hour {
            d.churn.min_written = (b as i128 * d.churn.window as i128 / HOUR as i128) as u64;
        }
        d
    }
}

/// How long a measured owner size is reused.
const SIZE_TTL: i64 = 6 * HOUR;

#[derive(Debug, Clone)]
pub struct Sample {
    pub point: SpacePoint,
    /// Bytes written per process name since the previous sample, largest
    /// first. Empty for the very first sample (no baseline yet).
    pub writes: Vec<(String, u64)>,
    /// Growth per owner folder, when an owners period ended with this sample.
    pub growth: Vec<(String, i64)>,
    pub period: Option<Period>,
    /// With a period: the directories with the most entries it listed.
    pub biggest: Vec<(String, usize)>,
    /// Incidents that hold, evaluated at the end of each owners period.
    pub incidents: Vec<Incident>,
    /// Those of them that started an episode now (announce these once).
    pub opened: Vec<Incident>,
}

/// What an owners period did.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Period {
    pub events: usize,
    /// Folders added because a heavy writer has a file open in them.
    pub open_files: usize,
    pub listed: usize,
    /// Entries in the folders listed: about 3.5 µs of CPU each.
    pub entries: usize,
    pub deferred: usize,
    pub unreadable: usize,
    /// Left alone: opening them would show a privacy prompt.
    pub skipped: usize,
    /// Directories whose size is remembered.
    pub tracked: usize,
    /// FSEvents history was incomplete (journal reset, or the replay timed
    /// out) so some growth may be missing.
    pub gaps: usize,
}

/// A process that wrote this much during a period is asked which files it
/// has open for writing, whose folders FSEvents may not report (see
/// [`proc::files_open_for_writing`]).
pub const OPEN_FILES_MIN: u64 = 4_000_000;

/// Free space that came back since the last sample (something deleted, the
/// Trash emptied) starts an owners period now, so the folder it came from
/// shows it within minutes rather than at the next period...
pub const EARLY_RISE: u64 = 1_000_000_000;
/// ...unless the last one is more recent than this. Only rises: they are a
/// few a day, while a busy disk drops by a GB in five minutes many times an
/// hour, which would multiply what the watcher costs.
const EARLY_AFTER: i64 = 10 * 60;

/// How long a replay of the journal may take before the rest is left to the
/// next period.
const REPLAY_TIMEOUT: Duration = Duration::from_secs(20);
/// Directories not changed for this long are forgotten.
const FORGET_AFTER: i64 = 7 * DAY;

struct Owners {
    tracker: Tracker,
    /// Path FSEvents watches.
    root: PathBuf,
    last_id: u64,
    /// Sample time of the previous period.
    last_period: Option<i64>,
    /// Wall clock of the previous period: files created after it are new.
    last_wall: i64,
    every: i64,
    /// Periods this process has run.
    periods: u64,
    /// The biggest directories the last period listed, with their entries.
    biggest: Vec<(String, usize)>,
}

/// What the watcher itself has cost, measured inside its samples only.
///
/// Process start-up is left out on purpose: averaged over a short run it
/// dominates and says nothing about the steady state (Idlewild's lesson).
/// Samples and owners periods are measured apart, since a period costs
/// far more and comes every few samples: an hour's cost is samples × their
/// cost plus periods × theirs, right for a run of any length. The first
/// period after a start is kept apart too (`catch_up`): it lists every
/// folder changed while the watcher wasn't running, 1.9 s once on a busy
/// Mac against 0.3 s for the next. It is counted in `cpu`, shown on its
/// own, and stands in for a period until a later one has been measured.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Cost {
    pub samples: u64,
    /// CPU time (user + system) spent sampling, periods included.
    pub cpu: Duration,
    /// Bytes this process wrote while sampling (its history included).
    pub written: u64,
    /// Owners periods after the first, and their CPU (listing folders and
    /// evaluating the incident clauses).
    pub periods: u64,
    pub period_cpu: Duration,
    /// CPU of the first owners period after the start.
    pub catch_up: Duration,
    /// Seconds between owners periods (0: growth per folder is off).
    pub every: i64,
}

impl Cost {
    /// CPU of a sample without an owners period, and bytes written per sample.
    pub fn per_sample(&self) -> (Duration, u64) {
        match self.samples {
            0 => (Duration::ZERO, 0),
            n => (
                self.cpu.saturating_sub(self.period_cpu + self.catch_up) / n as u32,
                self.written / n,
            ),
        }
    }

    /// CPU of an owners period, from those after the first; until one has
    /// run, the first stands in (the costliest).
    pub fn per_period(&self) -> Option<Duration> {
        match self.periods {
            0 => (self.catch_up > Duration::ZERO).then_some(self.catch_up),
            n => Some(self.period_cpu / n as u32),
        }
    }

    /// Steady-state cost per hour at one sample every `interval` seconds
    /// and one owners period every [`Cost::every`].
    pub fn per_hour(&self, interval: i64) -> (Duration, u64) {
        let (cpu, written) = self.per_sample();
        let samples = HOUR as f64 / interval.max(1) as f64;
        let periods = match self.every {
            0 => 0.0,
            every => HOUR as f64 / every as f64,
        };
        let period = self.per_period().unwrap_or_default();
        (
            cpu.mul_f64(samples) + period.mul_f64(periods),
            (written as f64 * samples) as u64,
        )
    }

    /// Account for one sample: `cpu` and `written` in all, `period` the CPU
    /// of its owners period if it had one (`first`: the first of the run).
    fn add(&mut self, cpu: Duration, written: u64, period: Option<(Duration, bool)>) {
        self.samples += 1;
        self.cpu += cpu;
        self.written += written;
        match period {
            Some((spent, true)) => self.catch_up += spent,
            Some((spent, false)) => {
                self.periods += 1;
                self.period_cpu += spent;
            }
            None => {}
        }
    }
}

impl Watcher {
    pub fn new(store: Store, volume: &Path) -> Result<Self> {
        // The watcher must never cost network traffic.
        fs::forbid_dataless_materialization()?;
        let key = fs::statfs(volume)
            .with_context(|| format!("statfs {}", volume.display()))?
            .mount_point;
        Ok(Watcher {
            store,
            volume: volume.to_path_buf(),
            key,
            prev: HashMap::new(),
            primed: false,
            last_compact: 0,
            cost: Cost::default(),
            owners: None,
            detect: Detect::default(),
            user: alerts::Settings::default(),
            sizes: HashMap::new(),
            period_writes: HashMap::new(),
            access_probes: Vec::new(),
            last_avail: None,
        })
    }

    /// Record whether the watcher has Full Disk Access (see
    /// [`recorded_access`]): without it, macOS keeps it out of other apps'
    /// folders and the growth there goes unseen.
    pub fn with_access_check(mut self, probes: Vec<PathBuf>) -> Self {
        self.access_probes = probes;
        self
    }

    pub fn with_detect(mut self, detect: Detect) -> Self {
        self.detect = detect;
        self.set_steady();
        self
    }

    /// A folder that changes by less than a fifth of the slowest growth an
    /// incident can have is steady, and is listed less often.
    fn set_steady(&mut self) {
        let g = self.detect.with_user(&self.user).growth;
        if let Some(o) = &mut self.owners {
            let per_period = g.min_growth as i128 * o.every as i128 / g.sustain.max(1) as i128;
            o.tracker.steady_below((per_period / 5) as u64);
        }
    }

    /// Also track growth per owner folder, replaying FSEvents every `every`
    /// seconds. Resumes from the journal position saved by the previous run,
    /// so changes made while the watcher wasn't running are counted too.
    pub fn with_owners(mut self, rules: Rules, every: i64) -> Result<Self> {
        let saved = |k: &str| -> Result<Option<i64>> {
            Ok(self
                .store
                .meta(&format!("owners.{}.{k}", self.key))?
                .and_then(|v| v.parse().ok()))
        };
        let (last_id, last_wall) = match (saved("id")?, saved("wall")?) {
            (Some(id), Some(wall)) => (id as u64, wall),
            _ => (fsevents::current_event_id(), unix_now()),
        };
        // The boot volume's paths are reported through its firmlinks
        // (`/Users`, `/private`, ...), so watch from the root.
        let root = if self.key == DATA_ROOT {
            PathBuf::from("/")
        } else {
            PathBuf::from(&self.key)
        };
        self.user = alerts::Settings::load(&self.store)?;
        let mut tracker = Tracker::new(rules, fs::dev_of(&self.volume).ok(), every.max(1));
        if let Ok(id) = fs::fsid(&self.volume) {
            tracker.follow_moves(id);
        }
        self.owners = Some(Owners {
            tracker,
            root,
            last_id,
            last_period: None,
            last_wall,
            every: every.max(1),
            periods: 0,
            biggest: Vec::new(),
        });
        self.cost.every = every.max(1);
        self.set_steady();
        Ok(self)
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The volume's mount point, the key its history is stored under.
    pub fn volume_key(&self) -> &str {
        &self.key
    }

    pub fn sample(&mut self, now: i64) -> Result<Sample> {
        let (cpu0, written0) = proc::own_usage();
        let st = fs::statfs(&self.volume)?;
        let point = SpacePoint {
            ts: now,
            total: st.total,
            avail: st.avail,
        };
        let writes = self.write_deltas(proc::own_processes());
        if let Some(o) = self.owners.as_mut() {
            // The first sample starts the first period.
            o.last_period.get_or_insert(now);
        }
        let rose = self
            .last_avail
            .is_some_and(|before| st.avail >= before.saturating_add(EARLY_RISE));
        self.last_avail = Some(st.avail);
        let due = self
            .owners
            .as_ref()
            .is_some_and(|o| o.due(now) || (rose && o.since(now) >= EARLY_AFTER));
        // Checked before a period lists anything: without Full Disk Access
        // the folders that would show a privacy prompt are left alone, since
        // nobody may be there to answer it and the open would wait.
        let access = match self.access_probes.as_slice() {
            [] => None,
            probes => {
                let fda = mdif_macos::sys::full_disk_access(probes);
                privacy::avoid_prompts(fda != Some(true));
                Some(match fda {
                    Some(true) => "1",
                    Some(false) => "0",
                    None => "?",
                })
            }
        };
        if due {
            let user = alerts::Settings::load(&self.store)?;
            if user != self.user {
                self.user = user;
                self.set_steady();
            }
        }
        let (growth, period) = match self.owners.as_mut() {
            Some(o) if due => {
                let heavy: Vec<i32> = self
                    .period_writes
                    .drain()
                    .filter(|(_, b)| *b >= OPEN_FILES_MIN)
                    .map(|(pid, _)| pid)
                    .collect();
                let first = o.periods == 0;
                let before = proc::own_usage().0;
                let (g, p) = o.period(now, &heavy)?;
                let spent = proc::own_usage().0.saturating_sub(before);
                (g, Some((p, spent, first)))
            }
            _ => (Vec::new(), None),
        };
        let spent = period.map(|(_, spent, first)| (spent, first));
        let period = period.map(|(p, _, _)| p);
        let mut meta = Vec::new();
        if let (Some(o), Some(p), Some((cpu, _))) = (&self.owners, period, spent) {
            let last = LastPeriod {
                at: now,
                listed: p.listed,
                entries: p.entries,
                deferred: p.deferred,
                skipped: p.skipped,
                cpu_us: cpu.as_micros() as u64,
                biggest: o.biggest.clone(),
            };
            meta.push((PERIOD_KEY.to_string(), serde_json::to_string(&last)?));
        }
        if let (Some(o), Some(_)) = (&self.owners, period) {
            meta.push((format!("owners.{}.id", self.key), o.last_id.to_string()));
            meta.push((format!("owners.{}.wall", self.key), o.last_wall.to_string()));
        }
        if let Some(granted) = access {
            meta.push((ACCESS_KEY.to_string(), format!("{now} {granted}")));
        }

        // The cost stored with this sample includes it, except for the
        // commit itself, which is counted in the in-memory total below and
        // so reaches the store with the next sample.
        let (cpu1, written1) = proc::own_usage();
        let mut so_far = self.cost;
        so_far.add(
            cpu1.saturating_sub(cpu0),
            written1.saturating_sub(written0),
            spent,
        );
        meta.push(("watch.cost".to_string(), format_cost(&so_far)));
        let meta: Vec<(&str, &str)> = meta.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        self.store
            .record(&self.key, point, &writes, &growth, &meta)?;
        if now - self.last_compact >= HOUR {
            self.store.compact(now)?;
            self.last_compact = now;
        }
        let before = proc::own_usage().0;
        let (incidents, opened) = match (&self.owners, period) {
            (Some(o), Some(_)) => self.detect(now, point.avail, 2 * o.every)?,
            _ => (Vec::new(), Vec::new()),
        };

        let (cpu2, written2) = proc::own_usage();
        // The incident clauses are part of the period's cost.
        let detect = cpu2.saturating_sub(before);
        self.cost.add(
            cpu2.saturating_sub(cpu0),
            written2.saturating_sub(written0),
            spent.map(|(s, first)| (s + detect, first)),
        );
        let biggest = match (&self.owners, period) {
            (Some(o), Some(_)) => o.biggest.clone(),
            _ => Vec::new(),
        };
        Ok(Sample {
            point,
            writes,
            growth,
            period,
            biggest,
            incidents,
            opened,
        })
    }

    /// Evaluate the incident clauses on the history and record episodes.
    fn detect(
        &mut self,
        now: i64,
        avail: u64,
        grace: i64,
    ) -> Result<(Vec<Incident>, Vec<Incident>)> {
        let Detect { growth, churn } = &self.detect.with_user(&self.user);
        let series = self.store.growth_series(now - growth.sustain - HOUR)?;
        let sizes = &mut self.sizes;
        let mut found = incidents::growth(
            &series,
            now,
            avail,
            |owner| {
                // Measured a while ago: add what it grew since.
                if let Some((at, size)) = sizes.get(owner)
                    && now - at < SIZE_TTL
                {
                    let since: i64 = series
                        .get(owner)
                        .map(|rows| rows.iter().filter(|(ts, _)| ts > at).map(|(_, b)| b).sum())
                        .unwrap_or(0);
                    return size.map(|s| (s as i64 + since).max(0) as u64);
                }
                let opts = scan::Options {
                    max_depth: 0,
                    min_node: u64::MAX,
                    big_files: None,
                };
                let size = scan::scan(Path::new(owner), &opts)
                    .ok()
                    .map(|s| s.root.totals.allocated);
                sizes.insert(owner.to_string(), (now, size));
                size
            },
            growth,
        );
        sizes.retain(|_, (at, _)| now - *at < SIZE_TTL);
        let writes = self.store.writes_series(now - churn.window - HOUR)?;
        let space = self.store.space_since(&self.key, now - churn.window)?;
        found.extend(incidents::churn(&writes, &space, now, churn));
        let opened = self.store.update_incidents(now, &found, grace)?;
        Ok((found, opened))
    }

    pub fn cost(&self) -> Cost {
        self.cost
    }

    fn write_deltas(&mut self, procs: Vec<proc::ProcIo>) -> Vec<(String, u64)> {
        let mut by_name: HashMap<&str, u64> = HashMap::new();
        for p in &procs {
            let written = match self.prev.get(&p.pid) {
                Some((name, before)) if *name == p.name && p.bytes_written >= *before => {
                    p.bytes_written - before
                }
                // Same pid, different process: it started since the last sample.
                Some(_) => p.bytes_written,
                // Started since the last sample, so all its writes are new.
                None if self.primed => p.bytes_written,
                None => 0,
            };
            if written > 0 {
                *by_name.entry(&p.name).or_default() += written;
                if self.owners.is_some() {
                    *self.period_writes.entry(p.pid).or_default() += written;
                }
            }
        }
        let mut out = Vec::new();
        let mut others = 0;
        for (name, bytes) in by_name {
            if bytes >= MIN_WRITER {
                out.push((name.to_string(), bytes));
            } else {
                others += bytes;
            }
        }
        if others > 0 {
            out.push((OTHERS.to_string(), others));
        }
        out.sort_by_key(|(_, b)| Reverse(*b));
        let primed = self.primed;
        self.prev = procs
            .into_iter()
            .map(|p| (p.pid, (p.name, p.bytes_written)))
            .collect();
        self.primed = true;
        if primed { out } else { Vec::new() }
    }
}

impl Owners {
    fn due(&self, now: i64) -> bool {
        self.since(now) >= self.every
    }

    /// Seconds since the previous period (0 before the first sample).
    fn since(&self, now: i64) -> i64 {
        self.last_period.map_or(0, |last| now - last)
    }

    /// Replay the journal since the previous period, add the folders of
    /// files the `heavy` writers have open, and update the sizes.
    fn period(&mut self, now: i64, heavy: &[i32]) -> Result<(Vec<(String, i64)>, Period)> {
        let wall = unix_now();
        let mut stream = Stream::start(&[&self.root], self.last_id, Duration::from_millis(50))
            .context("starting FSEvents")?;
        let deadline = std::time::Instant::now() + REPLAY_TIMEOUT;
        let mut events = Vec::new();
        let mut done = false;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match stream.next_timeout(left) {
                Some(e) if e.flags & flag::HISTORY_DONE != 0 => {
                    done = true;
                    break;
                }
                Some(e) => events.push(e),
                None => break,
            }
        }
        drop(stream);
        let replayed = events.len();
        if let Some(id) = events.iter().map(|e| e.id).max() {
            self.last_id = id;
        }
        let mut open_dirs: Vec<PathBuf> = heavy
            .iter()
            .flat_map(|&pid| proc::files_open_for_writing(pid))
            .filter_map(|f| f.parent().map(Path::to_path_buf))
            .collect();
        open_dirs.sort();
        open_dirs.dedup();
        let open_files = open_dirs.len();
        events.extend(open_dirs.into_iter().map(|path| fsevents::Event {
            path,
            flags: 0,
            id: 0,
        }));
        let up = self.tracker.apply(&events, self.last_wall, now);
        self.last_wall = wall;
        self.last_period = Some(now);
        self.periods += 1;
        self.tracker.prune(now - FORGET_AFTER);
        let period = Period {
            events: replayed,
            open_files,
            listed: up.listed,
            entries: up.entries,
            deferred: up.deferred,
            unreadable: up.unreadable,
            skipped: up.skipped,
            tracked: self.tracker.dirs_tracked(),
            gaps: up.gaps + usize::from(!done),
        };
        self.biggest = up
            .biggest
            .iter()
            .map(|(d, n)| (d.display().to_string(), *n))
            .collect();
        let growth = up
            .growth
            .into_iter()
            .map(|(o, d)| (o.display().to_string(), d))
            .collect();
        Ok((growth, period))
    }
}

const ACCESS_KEY: &str = "watch.full_disk_access";
const PERIOD_KEY: &str = "watch.last_period";

/// The last owners period, as recorded: what it listed and what it cost.
/// Listing is nearly all of a period's cost, so this says where it went.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LastPeriod {
    pub at: i64,
    pub listed: usize,
    pub entries: usize,
    pub deferred: usize,
    pub skipped: usize,
    pub cpu_us: u64,
    /// The directories with the most entries, most first.
    pub biggest: Vec<(String, usize)>,
}

pub fn recorded_period(store: &Store) -> Result<Option<LastPeriod>> {
    Ok(store
        .meta(PERIOD_KEY)?
        .and_then(|v| serde_json::from_str(&v).ok()))
}

/// When the watcher last checked for Full Disk Access, and whether it had it
/// (`None`: it couldn't tell).
pub fn recorded_access(store: &Store) -> Result<Option<(i64, Option<bool>)>> {
    let Some(v) = store.meta(ACCESS_KEY)? else {
        return Ok(None);
    };
    let mut it = v.split(' ');
    let at = it.next().and_then(|t| t.parse().ok());
    let granted = match it.next() {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    };
    Ok(at.map(|at| (at, granted)))
}

/// Seconds since the Unix epoch.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Read the cost the last running watcher recorded (see [`Watcher::sample`]).
pub fn recorded_cost(store: &Store) -> Result<Option<Cost>> {
    let Some(v) = store.meta("watch.cost")? else {
        return Ok(None);
    };
    Ok(parse_cost(&v))
}

/// `samples cpu_us written periods period_cpu_us catch_up_us every`; older
/// watchers wrote the first three only.
fn format_cost(c: &Cost) -> String {
    format!(
        "{} {} {} {} {} {} {}",
        c.samples,
        c.cpu.as_micros(),
        c.written,
        c.periods,
        c.period_cpu.as_micros(),
        c.catch_up.as_micros(),
        c.every
    )
}

fn parse_cost(v: &str) -> Option<Cost> {
    let n: Vec<u64> = v.split(' ').filter_map(|x| x.parse().ok()).collect();
    let at = |i: usize| n.get(i).copied().unwrap_or(0);
    matches!(n.len(), 3 | 7).then(|| Cost {
        samples: n[0],
        cpu: Duration::from_micros(n[1]),
        written: n[2],
        periods: at(3),
        period_cpu: Duration::from_micros(at(4)),
        catch_up: Duration::from_micros(at(5)),
        every: at(6) as i64,
    })
}

/// Sample every `interval`, aligned to the wall clock so buckets line up,
/// until `stop` returns true. `on_sample` sees every sample; errors in a
/// sample are reported to it and the loop carries on.
pub fn run(
    watcher: &mut Watcher,
    interval: Duration,
    mut stop: impl FnMut() -> bool,
    mut on_sample: impl FnMut(Result<&Sample>),
) {
    let step = interval.as_secs().max(1) as i64;
    while !stop() {
        let now = unix_now();
        match watcher.sample(now) {
            Ok(s) => on_sample(Ok(&s)),
            Err(e) => on_sample(Err(e)),
        }
        let next = (now / step + 1) * step;
        let wait = (next - unix_now()).max(1) as u64;
        std::thread::sleep(Duration::from_secs(wait));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_space_and_measures_its_own_writes() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = Watcher::new(Store::open_in_memory().unwrap(), dir.path()).unwrap();
        let first = w.sample(1_000).unwrap();
        assert!(first.writes.is_empty(), "no baseline on the first sample");
        assert!(first.point.avail > 0 && first.point.avail <= first.point.total);

        // This test process writes 8 MB between samples.
        let f = dir.path().join("w.bin");
        std::fs::write(&f, vec![1u8; 8 << 20]).unwrap();
        std::fs::File::open(&f).unwrap().sync_all().unwrap();
        let second = w.sample(1_300).unwrap();
        let me = mdif_macos::proc::own_processes()
            .into_iter()
            .find(|p| p.pid == std::process::id() as i32)
            .unwrap()
            .name;
        let mine: u64 = second
            .writes
            .iter()
            .filter(|(n, _)| *n == me)
            .map(|(_, b)| b)
            .sum();
        assert!(mine >= 8 << 20, "{:?}", second.writes);

        let pts = w.store().space_since(w.volume_key(), 0).unwrap();
        assert_eq!(pts.len(), 2);
        let cost = recorded_cost(w.store()).unwrap().unwrap();
        assert_eq!(cost.samples, 2);
    }

    #[test]
    fn an_hour_is_samples_plus_periods_whatever_the_run_length() {
        let ms = Duration::from_millis;
        // A 76-minute run: 16 samples of 5 ms, the catch-up period (1.9 s)
        // and one more period (300 ms).
        let mut c = Cost {
            every: 1800,
            ..Cost::default()
        };
        for i in 0..16 {
            let period = match i {
                6 => Some((ms(1900), true)),
                12 => Some((ms(300), false)),
                _ => None,
            };
            let extra = period.map_or(ms(0), |(p, _)| p);
            c.add(ms(5) + extra, 40_000, period);
        }
        assert_eq!(c.per_sample(), (ms(5), 40_000));
        assert_eq!(c.per_period(), Some(ms(300)));
        // 12 samples and 2 periods an hour.
        assert_eq!(c.per_hour(300), (ms(660), 480_000));
        // Stored and read back.
        assert_eq!(parse_cost(&format_cost(&c)), Some(c));
        // Before a second period, the catch-up stands in for one.
        let mut early = Cost {
            every: 1800,
            ..Cost::default()
        };
        early.add(ms(1905), 1, Some((ms(1900), true)));
        assert_eq!(early.per_period(), Some(ms(1900)));
        // An old three-field record still reads.
        let old = parse_cost("2 1000 10").unwrap();
        assert_eq!(old.per_sample().0, Duration::from_micros(500));
        assert!(parse_cost("garbage").is_none());
    }

    #[test]
    fn the_users_sizes_replace_the_defaults_and_nothing_else() {
        let d = Detect::default();
        assert_eq!(
            d.with_user(&alerts::Settings::default()).growth.min_growth,
            incidents::MIN_GROWTH
        );
        let s = alerts::Settings {
            growth_min: Some(5_000_000_000),
            churn_per_hour: Some(2_000_000_000),
            ..alerts::Settings::default()
        };
        let u = d.with_user(&s);
        assert_eq!(u.growth.min_growth, 5_000_000_000);
        assert_eq!(u.churn.min_written, 2_000_000_000);
        assert_eq!(
            (u.growth.min_r2, u.growth.min_share, u.growth.horizon),
            (d.growth.min_r2, d.growth.min_share, d.growth.horizon)
        );
        assert_eq!(
            (u.churn.min_quarters, u.churn.max_kept),
            (d.churn.min_quarters, d.churn.max_kept)
        );
        // Per hour: a half-hour window needs half as much.
        let half = Detect {
            churn: ChurnParams {
                window: HOUR / 2,
                ..ChurnParams::default()
            },
            ..Detect::default()
        };
        assert_eq!(half.with_user(&s).churn.min_written, 1_000_000_000);
    }

    #[test]
    fn full_disk_access_is_recorded_at_every_sample() {
        let dir = tempfile::tempdir().unwrap();
        let probe = dir.path().join("TCC.db");
        let mut w = Watcher::new(Store::open_in_memory().unwrap(), dir.path())
            .unwrap()
            .with_access_check(vec![probe.clone()]);
        w.sample(1_000).unwrap();
        // No database: can't tell.
        assert_eq!(recorded_access(w.store()).unwrap(), Some((1_000, None)));
        // A grant is noticed at the next sample.
        std::fs::write(&probe, b"").unwrap();
        w.sample(1_300).unwrap();
        assert_eq!(
            recorded_access(w.store()).unwrap(),
            Some((1_300, Some(true)))
        );
        let plain = Watcher::new(Store::open_in_memory().unwrap(), dir.path()).unwrap();
        assert_eq!(recorded_access(plain.store()).unwrap(), None);
    }
}
