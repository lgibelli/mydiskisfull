//! End to end on Windows: a real filler on a real NTFS volume, watched by the
//! real watcher. The volume is a VHDX that `diskpart` makes and mounts in a
//! folder (which is what a volume with no drive letter looks like to the
//! journal and to the volume lookups), so it needs an administrator, as a CI
//! runner is.
//!
//! A filler thread grows a file at a fixed rate, and halfway through a 256 MB
//! file in another folder is deleted, a one-off cleanup. The watcher samples
//! once a second and each second stands for five minutes of history; every
//! measurement (free space, the NTFS journal, folder sizes) is real, only the
//! clock is compressed.
//!
//! The forecast must match the filler's rate and take the deletion out as a
//! step; the filler's folder must be the one growth incident; and the cleanup
//! must be remembered with nothing grown back. No process is blamed for
//! writes: Windows' per-process write counter includes pipes, so the watcher
//! records none there, and nothing is claimed about a churner.
//!
//! ```sh
//! cargo test -p mdif-core --test e2e_watch_windows -- --ignored --nocapture
//! ```

#![cfg(windows)]

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mdif_core::forecast::{self, DAY, HOUR, Params};
use mdif_core::incidents::{GrowthParams, Kind};
use mdif_core::owners::{self, Rules};
use mdif_core::store::Store;
use mdif_core::watch::{Detect, Watcher};

const MB: u64 = 1 << 20;
/// Seconds of history each real second stands for.
const WARP: f64 = 300.0;
const SAMPLES: usize = 48;
/// Seconds of history between owners periods (3 real seconds).
const PERIOD: i64 = 900;
const TICK: Duration = Duration::from_millis(250);
const FILL_CHUNK: u64 = MB / 2;
const SEED: u64 = 256 * MB;

fn detect() -> Detect {
    Detect {
        growth: GrowthParams {
            sustain: 2 * HOUR,
            grid: PERIOD,
            min_growth: 10 * MB,
            ..GrowthParams::default()
        },
        ..Detect::default()
    }
}

/// A VHDX attached and formatted NTFS, mounted at a folder; detached on drop.
struct Disk {
    file: PathBuf,
    mount: PathBuf,
    attached: bool,
}

impl Disk {
    fn attach(dir: &Path) -> Disk {
        let file = dir.join("e2e.vhdx");
        let mount = dir.join("mnt");
        fs::create_dir(&mount).unwrap();
        // Armed before anything runs: a disk that is attached and then fails to
        // format is still detached.
        let disk = Disk {
            file,
            mount,
            attached: true,
        };
        // It has a letter for a moment: `fsutil` takes one, and a new volume has
        // no change journal until something makes one (Windows makes one on the
        // system drive, and a program may on others). Then only the folder is left,
        // which is what a volume with no letter looks like.
        diskpart(&format!(
            "create vdisk file=\"{}\" maximum=1536 type=expandable\n\
             select vdisk file=\"{}\"\n\
             attach vdisk\n\
             create partition primary\n\
             format fs=ntfs quick label=mdife2e\n\
             assign letter=Y\n\
             assign mount=\"{}\"\n",
            disk.file.display(),
            disk.file.display(),
            disk.mount.display()
        ));
        let out = Command::new("fsutil")
            .args(["usn", "createjournal", "m=33554432", "a=4194304", "Y:"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "fsutil: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        diskpart("select volume Y\nremove letter=Y\n");
        disk
    }

    /// Detaches it, and says so if that fails (a test that passed shouldn't leave
    /// a disk attached).
    fn detach(mut self) {
        self.attached = false;
        diskpart(&format!(
            "select vdisk file=\"{}\"\ndetach vdisk\n",
            self.file.display()
        ));
    }
}

impl Drop for Disk {
    fn drop(&mut self) {
        // Only while unwinding from a failure: best effort.
        if self.attached {
            let script = format!(
                "select vdisk file=\"{}\"\ndetach vdisk\n",
                self.file.display()
            );
            let _ = std::panic::catch_unwind(|| diskpart(&script));
        }
    }
}

fn diskpart(script: &str) {
    let file = std::env::temp_dir().join(format!("mdif-e2e-{}.diskpart", std::process::id()));
    fs::write(&file, script).unwrap();
    let out = Command::new("diskpart")
        .arg("/s")
        .arg(&file)
        .output()
        .unwrap();
    let _ = fs::remove_file(&file);
    let text = String::from_utf8_lossy(&out.stdout);
    eprintln!("{text}");
    assert!(
        out.status.success() && !text.contains("DiskPart has encountered an error"),
        "diskpart failed: {text} {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write_synced(path: &Path, len: u64) {
    let mut f = fs::File::create(path).unwrap();
    // Not one repeated byte: a compressed volume would hold less than it is told.
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let block: Vec<u8> = (0..MB as usize)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    for _ in 0..len / MB {
        f.write_all(&block).unwrap();
    }
    f.sync_all().unwrap();
}

#[test]
#[ignore = "creates and mounts a VHDX with diskpart (an administrator); run with --ignored"]
fn detects_a_filler_and_a_cleanup_on_a_real_ntfs_volume() {
    let tmp = tempfile::tempdir().unwrap();
    let disk = Disk::attach(tmp.path());
    let vol = disk.mount.clone();
    for d in ["fill", "seed"] {
        fs::create_dir(vol.join(d)).unwrap();
    }
    let seed = vol.join("seed/seed.bin");
    write_synced(&seed, SEED);
    let filled = vol.join("fill/fill.bin");

    let store = Store::open(&tmp.path().join("history.sqlite3")).unwrap();
    let w = Watcher::new(store, &vol).unwrap();
    let key = w.volume_key().to_string();
    assert!(
        key.starts_with(r"\\?\Volume{"),
        "a volume with no letter is kept by its identity: {key}"
    );
    let mut w = w
        .with_owners(Rules::default().depth(&key, 1), PERIOD)
        .unwrap()
        .with_detect(detect());
    let owner = |d: &str| format!("{}{d}", key);

    // The filler: its own thread, appending at a fixed rate with a deadline
    // clock, so the rate holds whatever each tick costs.
    let stop = Arc::new(AtomicBool::new(false));
    let filler = {
        let (stop, path) = (stop.clone(), filled.clone());
        std::thread::spawn(move || {
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            let chunk = vec![0xa5u8; FILL_CHUNK as usize];
            let start = Instant::now();
            let mut tick = 0u32;
            while !stop.load(Ordering::Relaxed) {
                f.write_all(&chunk).unwrap();
                f.sync_data().unwrap();
                tick += 1;
                let next = start + TICK * tick;
                std::thread::sleep(next.saturating_duration_since(Instant::now()));
            }
        })
    };

    let mut truth: Vec<(i64, u64)> = Vec::new();
    let fake0 = 1_700_000_100;
    let t0 = Instant::now();
    for i in 0..SAMPLES {
        let ts = fake0 + (t0.elapsed().as_secs_f64() * WARP) as i64;
        let s = w.sample(ts).unwrap();
        if let Some(p) = s.period {
            eprintln!("period at {i}: {p:?}, growth {:?}", s.growth);
        }
        for inc in &s.opened {
            eprintln!("incident at {i}: {}", inc.sentence(None));
        }
        truth.push((ts, fs::metadata(&filled).map_or(0, |m| m.len())));
        // Known to the tracker before the cleanup, then cleaned up.
        if i == 2 {
            fs::write(vol.join("seed/marker"), b"x").unwrap();
        }
        if i == SAMPLES / 2 {
            fs::remove_file(&seed).unwrap();
        }
        let next = t0 + Duration::from_secs(i as u64 + 1);
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    stop.store(true, Ordering::Relaxed);
    filler.join().unwrap();

    let pts = w.store().space_since(&key, 0).unwrap();
    let params = Params {
        step_min: 64 * MB,
        ..Params::default()
    };
    let f = forecast::forecast(&pts, &params).expect("a forecast");
    let (t_first, len_first) = truth[0];
    let (t_last, len_last) = truth[truth.len() - 1];
    let filler_per_day = (len_last - len_first) as f64 / (t_last - t_first) as f64 * DAY as f64;
    let rate = f.rate.expect("a rate");
    eprintln!(
        "filler grew {} B/day (warped), forecast {rate:?}",
        filler_per_day as u64
    );
    eprintln!("steps {:?}", f.steps);
    let ratio = rate.per_day / filler_per_day;
    assert!(
        (0.8..1.25).contains(&ratio),
        "forecast {:.0} B/day vs filler {filler_per_day:.0} B/day",
        rate.per_day
    );
    assert!(f.full_in.is_some(), "a filling disk has a date");
    let freed: u64 = f.steps.iter().map(|s| s.freed).sum();
    assert!(
        (1..=2).contains(&f.steps.len()) && freed >= SEED * 8 / 10,
        "the seed deletion is the only step: {:?}",
        f.steps
    );
    assert_eq!(rate.steps_removed, f.steps.len());

    // Owners: the filler's folder grew by its file, the seed's lost the seed.
    let growth: HashMap<String, i64> = w.store().growth_since(0).unwrap().into_iter().collect();
    eprintln!("owner growth {growth:?}");
    let g = |d: &str| growth.get(&owner(d)).copied().unwrap_or(0);
    let final_len = fs::metadata(&filled).unwrap().len() as i64;
    assert!(
        (final_len * 7 / 10..=final_len + MB as i64).contains(&g("fill")),
        "fill folder grew {} B, its file is {final_len} B",
        g("fill")
    );
    let lost = -g("seed");
    assert!(
        (SEED as i64 * 19 / 20..=SEED as i64 * 21 / 20).contains(&lost),
        "seed folder lost {lost} B; the seed is {SEED} B"
    );
    let cleanups = owners::regrowth(&w.store().growth_series(0).unwrap(), 64 * MB);
    assert_eq!(cleanups.len(), 1, "{cleanups:?}");
    assert_eq!(cleanups[0].owner, owner("seed"));
    assert!(cleanups[0].share() < 0.01, "nothing grew back");

    // Incidents: the filler's folder, once.
    let episodes = w.store().incidents_since(0).unwrap();
    for e in &episodes {
        eprintln!("episode: {}", e.incident.sentence(None));
    }
    let ours: Vec<(&str, String)> = episodes
        .iter()
        .map(|e| (e.incident.kind.as_str(), e.incident.subject.clone()))
        .filter(|(_, s)| s.starts_with(&key))
        .collect();
    assert_eq!(
        ours,
        [(Kind::Growth.as_str(), owner("fill"))],
        "{episodes:?}"
    );
    drop(w);
    disk.detach();
}
