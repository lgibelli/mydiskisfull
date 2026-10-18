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

use crate::forecast::{DAY, HOUR};
use crate::incidents::{self, ChurnParams, GrowthParams, Incident};
use crate::map::DATA_ROOT;
use crate::owners::{Rules, Tracker};
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
    /// Sizes of owners measured for the growth clauses, and when.
    sizes: HashMap<String, (i64, Option<u64>)>,
    /// Bytes each process wrote since the last owners period.
    period_writes: HashMap<i32, u64>,
}

/// What counts as an incident. The defaults are the plan's clauses.
#[derive(Debug, Clone, Default)]
pub struct Detect {
    pub growth: GrowthParams,
    pub churn: ChurnParams,
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
    pub deferred: usize,
    pub unreadable: usize,
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
}

/// What the watcher itself has cost, measured inside its samples only.
///
/// Process start-up is left out on purpose: averaged over a short run it
/// dominates and says nothing about the steady state (Idlewild's lesson).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Cost {
    pub samples: u64,
    /// CPU time (user + system) spent sampling.
    pub cpu: Duration,
    /// Bytes this process wrote while sampling (its history included).
    pub written: u64,
}

impl Cost {
    pub fn per_sample(&self) -> (Duration, u64) {
        match self.samples {
            0 => (Duration::ZERO, 0),
            n => (self.cpu / n as u32, self.written / n),
        }
    }

    /// Steady-state cost per hour at one sample every `interval` seconds.
    pub fn per_hour(&self, interval: i64) -> (Duration, u64) {
        let (cpu, written) = self.per_sample();
        let per_hour = HOUR as f64 / interval.max(1) as f64;
        (cpu.mul_f64(per_hour), (written as f64 * per_hour) as u64)
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
            sizes: HashMap::new(),
            period_writes: HashMap::new(),
        })
    }

    pub fn with_detect(mut self, detect: Detect) -> Self {
        self.detect = detect;
        self
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
        self.owners = Some(Owners {
            tracker: Tracker::new(rules, fs::dev_of(&self.volume).ok(), every.max(1)),
            root,
            last_id,
            last_period: None,
            last_wall,
            every: every.max(1),
        });
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
        let (growth, period) = match self.owners.as_mut() {
            Some(o) if o.due(now) => {
                let heavy: Vec<i32> = self
                    .period_writes
                    .drain()
                    .filter(|(_, b)| *b >= OPEN_FILES_MIN)
                    .map(|(pid, _)| pid)
                    .collect();
                let (g, p) = o.period(now, &heavy)?;
                (g, Some(p))
            }
            _ => (Vec::new(), None),
        };
        let mut meta = Vec::new();
        if let (Some(o), Some(_)) = (&self.owners, period) {
            meta.push((format!("owners.{}.id", self.key), o.last_id.to_string()));
            meta.push((format!("owners.{}.wall", self.key), o.last_wall.to_string()));
        }

        // The cost stored with this sample includes it, except for the
        // commit itself, which is counted in the in-memory total below and
        // so reaches the store with the next sample.
        let (cpu1, written1) = proc::own_usage();
        let so_far = Cost {
            samples: self.cost.samples + 1,
            cpu: self.cost.cpu + cpu1.saturating_sub(cpu0),
            written: self.cost.written + written1.saturating_sub(written0),
        };
        meta.push((
            "watch.cost".to_string(),
            format!(
                "{} {} {}",
                so_far.samples,
                so_far.cpu.as_micros(),
                so_far.written
            ),
        ));
        let meta: Vec<(&str, &str)> = meta.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        self.store
            .record(&self.key, point, &writes, &growth, &meta)?;
        if now - self.last_compact >= HOUR {
            self.store.compact(now)?;
            self.last_compact = now;
        }
        let (incidents, opened) = match (&self.owners, period) {
            (Some(o), Some(_)) => self.detect(now, point.avail, 2 * o.every)?,
            _ => (Vec::new(), Vec::new()),
        };

        let (cpu2, written2) = proc::own_usage();
        self.cost.samples += 1;
        self.cost.cpu += cpu2.saturating_sub(cpu0);
        self.cost.written += written2.saturating_sub(written0);
        Ok(Sample {
            point,
            writes,
            growth,
            period,
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
        let Detect { growth, churn } = &self.detect;
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
        self.last_period
            .is_some_and(|last| now - last >= self.every)
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
        self.tracker.prune(now - FORGET_AFTER);
        let period = Period {
            events: replayed,
            open_files,
            listed: up.listed,
            deferred: up.deferred,
            unreadable: up.unreadable,
            tracked: self.tracker.dirs_tracked(),
            gaps: up.gaps + usize::from(!done),
        };
        let growth = up
            .growth
            .into_iter()
            .map(|(o, d)| (o.display().to_string(), d))
            .collect();
        Ok((growth, period))
    }
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
    let n: Vec<u64> = v.split(' ').filter_map(|x| x.parse().ok()).collect();
    Ok((n.len() == 3).then(|| Cost {
        samples: n[0],
        cpu: Duration::from_micros(n[1]),
        written: n[2],
    }))
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
}
