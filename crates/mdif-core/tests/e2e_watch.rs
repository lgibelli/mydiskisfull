//! End to end: real processes doing real I/O on a real APFS volume, watched
//! by the real watcher.
//!
//! A scratch APFS disk image gets three child processes, each in its own
//! folder:
//!
//! - a **filler** that grows a file at a fixed rate,
//! - a **churner** that rewrites a file in place, writing 8× more than the
//!   filler while taking no space,
//! - a **reader** that only reads, the decoy.
//!
//! Halfway through, a 256 MB file in a third folder is deleted, a one-off
//! cleanup.
//!
//! The watcher samples once a second, and each second stands for five
//! minutes of history so a forecast window fills in under a minute. Every
//! measurement (free space, per-process writes, FSEvents, folder sizes) is
//! real. Only the clock is compressed, and the incident thresholds are
//! scaled down to match.
//!
//! The forecast must match the filler's rate alone and take the deletion out
//! as a step; the filler's folder must be the one growth incident and the
//! churner the one churn incident; the cleanup must be remembered with
//! nothing grown back; and the reader must show up nowhere.
//!
//! Needs `hdiutil` and about 1 GB of free space, and takes a minute:
//!
//! ```sh
//! cargo test -p mdif-core --test e2e_watch -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use mdif_core::forecast::{self, DAY, HOUR, Params};
use mdif_core::incidents::{ChurnParams, GrowthParams, Kind};
use mdif_core::owners::{self, Rules};
use mdif_core::store::Store;
use mdif_core::watch::{Detect, Watcher};

const ROLE: &str = "MDIF_E2E_ROLE";
const TARGET: &str = "MDIF_E2E_PATH";
const LIFETIME: &str = "MDIF_E2E_SECS";
/// Where a role keeps the count of bytes it has written (ground truth).
const COUNT: &str = "MDIF_E2E_COUNT";

const MB: u64 = 1 << 20;
/// Seconds of history each real second stands for.
const WARP: f64 = 300.0;
const SAMPLES: usize = 48;
/// Seconds of history between owners periods (3 real seconds).
const PERIOD: i64 = 900;
/// Both writers act every `TICK`: the filler appends `FILL_CHUNK`, the
/// churner rewrites its whole `CHURN_SIZE` file.
const TICK: Duration = Duration::from_millis(250);
const FILL_CHUNK: u64 = MB / 2;
const CHURN_SIZE: u64 = 4 * MB;
const SEED: u64 = 256 * MB;

/// The plan's clauses, scaled to a minute of real time.
fn detect() -> Detect {
    Detect {
        growth: GrowthParams {
            sustain: 2 * HOUR,
            grid: PERIOD,
            min_growth: 10 * MB,
            ..GrowthParams::default()
        },
        churn: ChurnParams {
            min_written: 64 * MB,
            // The filler's growth counts against what the churner keeps.
            max_kept: 0.25,
            ..ChurnParams::default()
        },
    }
}

#[test]
#[ignore = "creates and mounts an APFS disk image; run with --ignored"]
fn detects_filler_and_churner_and_nothing_else() {
    let tmp = tempfile::tempdir().unwrap();
    let image = Image::attach(tmp.path());
    let vol = image.mount.clone();
    for d in ["fill", "churn", "seed"] {
        fs::create_dir(vol.join(d)).unwrap();
    }

    let seed = vol.join("seed/seed.bin");
    write_synced(&seed, SEED);
    let churned = vol.join("churn/churn.bin");
    write_synced(&churned, CHURN_SIZE);
    let filled = vol.join("fill/fill.bin");

    let store = Store::open(&tmp.path().join("history.sqlite3")).unwrap();
    let w = Watcher::new(store, &vol).unwrap();
    let key = w.volume_key().to_string();
    let mut w = w
        .with_owners(Rules::default().depth(&key, 1), PERIOD)
        .unwrap()
        .with_detect(detect());
    let owner = |d: &str| format!("{key}/{d}");

    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let lifetime = SAMPLES as u64 + 30;
    let _fill = Role::spawn(&bin, "fill", &filled, lifetime);
    let _churn = Role::spawn(&bin, "churn", &churned, lifetime);
    let _read = Role::spawn(&bin, "read", &churned, lifetime);
    let churn_count = bin.join("mdif-e2e-churn.count");

    // Ground truth, sampled alongside the watcher: how big the filler's file
    // is and how much the churner says it has written.
    let mut truth: Vec<(i64, u64)> = Vec::new();
    let mut churn_truth: Vec<u64> = Vec::new();
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
        churn_truth.push(
            fs::read_to_string(&churn_count)
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
        );
        // A change makes the cleanup folder known before the cleanup. Both
        // are done by other processes: FSEvents leaves out the watcher's own.
        if i == 2 {
            run("/usr/bin/touch", &vol.join("seed/marker"));
        }
        if i == SAMPLES / 2 {
            run("/bin/rm", &seed);
        }
        let next = t0 + Duration::from_secs(i as u64 + 1);
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }

    let pts = w.store().space_since(&key, 0).unwrap();
    let params = Params {
        step_min: 64 * MB,
        ..Params::default()
    };
    let f = forecast::forecast(&pts, &params).expect("a forecast");
    let writers: HashMap<String, u64> = w.store().writes_since(0).unwrap().into_iter().collect();

    let (t_first, len_first) = truth[0];
    let (t_last, len_last) = truth[truth.len() - 1];
    let filler_per_day = (len_last - len_first) as f64 / (t_last - t_first) as f64 * DAY as f64;
    let rate = f.rate.expect("a rate");
    eprintln!(
        "filler grew {} B/day (warped), forecast {rate:?}",
        filler_per_day as u64
    );
    eprintln!("steps {:?}", f.steps);
    let mut top: Vec<_> = writers.iter().collect();
    top.sort_by_key(|(_, b)| std::cmp::Reverse(**b));
    eprintln!("writers {:?}", &top[..top.len().min(10)]);

    // The filler is detected: the forecast is its rate, and only its rate
    // (counting the churner's writes as growth would make it 9× higher).
    let ratio = rate.per_day / filler_per_day;
    assert!(
        (0.8..1.25).contains(&ratio),
        "forecast {:.0} B/day vs filler {filler_per_day:.0} B/day",
        rate.per_day
    );
    assert!(f.full_in.is_some(), "a filling disk has a date");

    // The deletion is a one-off step, taken out of the trend.
    let freed: u64 = f.steps.iter().map(|s| s.freed).sum();
    assert!(
        (1..=2).contains(&f.steps.len()) && freed >= SEED * 8 / 10,
        "the seed deletion is the only step: {:?}",
        f.steps
    );
    assert_eq!(rate.steps_removed, f.steps.len());
    assert!(
        f.recent_full_in.is_none(),
        "a steady filler is not speeding up"
    );

    // The churner is detected as the heavy writer, the filler writes about
    // what it grows, and the reader writes nothing. Compared with what each
    // actually did between the first and last sample, not with its
    // schedule, which a slow disk can't always keep.
    let churn = writers.get("mdif-e2e-churn").copied().unwrap_or(0) as f64;
    let fill = writers.get("mdif-e2e-fill").copied().unwrap_or(0) as f64;
    let churn_expected = (churn_truth[churn_truth.len() - 1] - churn_truth[0]) as f64;
    let fill_expected = (len_last - len_first) as f64;
    assert!(
        (0.8 * churn_expected..1.5 * churn_expected).contains(&churn),
        "churner: watcher measured {churn} B, it wrote {churn_expected}"
    );
    assert!(
        (0.8 * fill_expected..3.0 * fill_expected).contains(&fill),
        "filler: watcher measured {fill} B, its file grew {fill_expected}"
    );
    assert!(churn > 2.0 * fill);
    assert!(
        !writers.contains_key("mdif-e2e-read"),
        "the reader only reads"
    );

    // Owners: the filler's folder grew by its file, the churner's didn't
    // grow, and the cleanup folder lost the seed.
    let growth: HashMap<String, i64> = w.store().growth_since(0).unwrap().into_iter().collect();
    eprintln!("owner growth {growth:?}");
    let g = |d: &str| growth.get(&owner(d)).copied().unwrap_or(0);
    let final_len = fs::metadata(&filled).unwrap().len() as i64;
    assert!(
        (final_len * 7 / 10..=final_len + MB as i64).contains(&g("fill")),
        "fill folder grew {} B, its file is {final_len} B",
        g("fill")
    );
    assert!(
        g("churn").abs() <= 2 * CHURN_SIZE as i64,
        "churn folder grew {}",
        g("churn")
    );
    assert!(
        (g("seed") + SEED as i64).abs() <= MB as i64,
        "seed folder lost {}",
        -g("seed")
    );
    let cleanups = owners::regrowth(&w.store().growth_series(0).unwrap(), 64 * MB);
    assert_eq!(cleanups.len(), 1, "{cleanups:?}");
    assert_eq!(cleanups[0].owner, owner("seed"));
    assert!(cleanups[0].share() < 0.01, "nothing grew back");

    // Incidents: exactly the filler's folder (growth) and the churner
    // (churn) among what this test controls. Other processes on the host
    // are outside its control and ignored.
    let episodes = w.store().incidents_since(0).unwrap();
    for e in &episodes {
        eprintln!("episode: {}", e.incident.sentence(None));
    }
    // One episode each: an incident that flickered open and closed would
    // show up twice.
    let mut ours: Vec<(&str, String)> = episodes
        .iter()
        .map(|e| (e.incident.kind.as_str(), e.incident.subject.clone()))
        .filter(|(_, s)| s.starts_with("mdif-e2e-") || s.starts_with(&key))
        .collect();
    ours.sort();
    assert_eq!(
        ours,
        [
            (Kind::Churn.as_str(), "mdif-e2e-churn".to_string()),
            (Kind::Growth.as_str(), owner("fill")),
        ],
        "{episodes:?}"
    );
}

/// Where the child processes start. A no-op unless spawned by the test.
#[test]
fn role() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let path = PathBuf::from(std::env::var(TARGET).unwrap());
    let secs: u64 = std::env::var(LIFETIME).unwrap().parse().unwrap();
    let count = std::env::var_os(COUNT).map(PathBuf::from);
    let mut written = 0u64;
    let end = Instant::now() + Duration::from_secs(secs);
    let start = Instant::now();
    let mut tick = 0u32;
    let mut out = match role.as_str() {
        "fill" => Some(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap(),
        ),
        "churn" => Some(OpenOptions::new().write(true).open(&path).unwrap()),
        _ => None,
    };
    let chunk = vec![0xa5u8; CHURN_SIZE.max(FILL_CHUNK) as usize];
    while Instant::now() < end {
        match (role.as_str(), out.as_mut()) {
            ("fill", Some(f)) => {
                f.write_all(&chunk[..FILL_CHUNK as usize]).unwrap();
                f.sync_data().unwrap();
            }
            ("churn", Some(f)) => {
                f.seek(SeekFrom::Start(0)).unwrap();
                f.write_all(&chunk[..CHURN_SIZE as usize]).unwrap();
                f.sync_data().unwrap();
                written += CHURN_SIZE;
            }
            ("read", _) => {
                std::hint::black_box(fs::read(&path).unwrap());
            }
            _ => panic!("unknown role {role}"),
        }
        if let Some(c) = &count {
            fs::write(c, written.to_string()).unwrap();
        }
        // Deadline-based, so the rate holds whatever each tick costs.
        tick += 1;
        let next = start + TICK * tick;
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
}

/// A child process running [`role`], under its own executable name so the
/// watcher can tell the roles apart. Killed on drop.
struct Role(Child);

impl Role {
    fn spawn(bin: &Path, role: &str, target: &Path, secs: u64) -> Role {
        let exe = bin.join(format!("mdif-e2e-{role}"));
        fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        let child = Command::new(&exe)
            .args(["role", "--exact", "--test-threads", "1"])
            .env(ROLE, role)
            .env(TARGET, target)
            .env(LIFETIME, secs.to_string())
            .env(COUNT, bin.join(format!("mdif-e2e-{role}.count")))
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        Role(child)
    }
}

impl Drop for Role {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A 1 GB sparse APFS image, detached on drop.
///
/// It is mounted browsable: fseventsd keeps no journal for `-nobrowse`
/// volumes, and the watcher replays the journal. Spotlight is kept out with
/// a `.metadata_never_index` marker, written in a first, hidden mount so
/// indexing never starts.
struct Image {
    mount: PathBuf,
}

impl Image {
    fn attach(dir: &Path) -> Image {
        let img = dir.join("e2e.sparseimage");
        let mount = dir.join("mnt");
        hdiutil(&[
            "create".as_ref(),
            "-quiet".as_ref(),
            "-size".as_ref(),
            "1g".as_ref(),
            "-fs".as_ref(),
            "APFS".as_ref(),
            "-volname".as_ref(),
            "mdif-e2e".as_ref(),
            "-type".as_ref(),
            "SPARSE".as_ref(),
            img.as_os_str(),
        ]);
        let attach = |browse: bool| {
            let mut args: Vec<&OsStr> = vec!["attach".as_ref(), "-quiet".as_ref()];
            if !browse {
                args.push("-nobrowse".as_ref());
            }
            args.extend::<[&OsStr; 4]>([
                "-noautoopen".as_ref(),
                "-mountpoint".as_ref(),
                mount.as_os_str(),
                img.as_os_str(),
            ]);
            hdiutil(&args);
        };
        attach(false);
        fs::write(mount.join(".metadata_never_index"), b"").unwrap();
        hdiutil(&["detach".as_ref(), "-quiet".as_ref(), mount.as_os_str()]);
        attach(true);
        Image { mount }
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        for _ in 0..5 {
            let ok = Command::new("hdiutil")
                .args(["detach", "-quiet", "-force"])
                .arg(&self.mount)
                .status()
                .is_ok_and(|s| s.success());
            if ok {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        eprintln!("could not detach {}", self.mount.display());
    }
}

fn hdiutil(args: &[&OsStr]) {
    let status = Command::new("hdiutil").args(args).status().unwrap();
    assert!(status.success(), "hdiutil {args:?} failed");
}

fn run(cmd: &str, arg: &Path) {
    let ok = Command::new(cmd).arg(arg).status().unwrap().success();
    assert!(ok, "{cmd} {}", arg.display());
}

fn write_synced(path: &Path, len: u64) {
    let mut f = File::create(path).unwrap();
    let chunk = vec![0x5au8; MB as usize];
    for _ in 0..len / MB {
        f.write_all(&chunk).unwrap();
    }
    f.sync_all().unwrap();
}
