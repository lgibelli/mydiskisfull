//! The watcher: one cheap sample every few minutes.
//!
//! A sample is one `statfs` of the watched volume plus one `proc_pid_rusage`
//! per process of the user, a few milliseconds of CPU. Write counters are
//! cumulative per process, so each sample stores what every process wrote
//! since the previous sample, summed per process name. Small writers are
//! pooled as "(others)" to keep the history bounded.
//!
//! The watcher records its own CPU time and writes, so its budget ("a monitor
//! must never become the thing it hunts") is measured, not assumed.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use mdif_macos::{fs, proc};

use crate::forecast::HOUR;
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
}

#[derive(Debug, Clone)]
pub struct Sample {
    pub point: SpacePoint,
    /// Bytes written per process name since the previous sample, largest
    /// first. Empty for the very first sample (no baseline yet).
    pub writes: Vec<(String, u64)>,
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
        })
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

        // The cost stored with this sample includes it, except for the
        // commit itself, which is counted in the in-memory total below and
        // so reaches the store with the next sample.
        let (cpu1, written1) = proc::own_usage();
        let so_far = Cost {
            samples: self.cost.samples + 1,
            cpu: self.cost.cpu + cpu1.saturating_sub(cpu0),
            written: self.cost.written + written1.saturating_sub(written0),
        };
        let meta = format!(
            "{} {} {}",
            so_far.samples,
            so_far.cpu.as_micros(),
            so_far.written
        );
        self.store
            .record(&self.key, point, &writes, &[("watch.cost", &meta)])?;
        if now - self.last_compact >= HOUR {
            self.store.compact(now)?;
            self.last_compact = now;
        }

        let (cpu2, written2) = proc::own_usage();
        self.cost.samples += 1;
        self.cost.cpu += cpu2.saturating_sub(cpu0);
        self.cost.written += written2.saturating_sub(written0);
        Ok(Sample { point, writes })
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
