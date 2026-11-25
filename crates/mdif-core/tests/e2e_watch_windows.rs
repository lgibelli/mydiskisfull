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
    /// The disk's file, the folder it's mounted at, and DiskPart's scripts.
    admin: AdminOnly,
    file: PathBuf,
    mount: PathBuf,
    /// The scripts run so far, each from a file of its own.
    runs: u32,
    /// When the last script ended: DiskPart wants 15 s between two.
    last: Option<Instant>,
    attached: bool,
}

impl Disk {
    fn attach() -> Disk {
        let admin = AdminOnly::new();
        let file = admin.0.join("e2e.vhdx");
        let mount = admin.0.join("mnt");
        fs::create_dir(&mount).unwrap();
        // Armed before anything runs: a disk that is attached and then fails to
        // format is still detached.
        let mut disk = Disk {
            admin,
            file,
            mount,
            runs: 0,
            // DiskPart's 15 s are between any two scripts, another test's too,
            // which may just have ended.
            last: Some(Instant::now()),
            attached: true,
        };
        // It has a letter for a moment: `fsutil` takes one, and a new volume has
        // no change journal until something makes one (Windows makes one on the
        // system drive, and a program may on others). Then only the folder is left,
        // which is what a volume with no letter looks like.
        // A letter nothing else uses: picked here, assigned by DiskPart, so two of
        // these tests at once could pick the same, and one fail at `assign` (only
        // that: the label tells another disk). Cargo runs test programs one after
        // another, CI this one and the Recycle Bin's in steps of their own.
        let letter = ('D'..='Z')
            .rev()
            .find(|l| !logical_drives().contains(l))
            .expect("a free drive letter");
        // A label of its own, made at random: what tells that the letter names
        // this disk when it's used.
        let label = format!("mdif{:08x}", random() as u32);
        disk.diskpart(&format!(
            "create vdisk file=\"{}\" maximum=1536 type=expandable\n\
             select vdisk file=\"{}\"\n\
             attach vdisk\n\
             create partition primary\n\
             format fs=ntfs quick label={label}\n\
             assign letter={letter}\n\
             assign mount=\"{}\"\n",
            disk.file.display(),
            disk.file.display(),
            disk.mount.display()
        ));
        let root = format!(r"{letter}:\");
        assert_eq!(label_of(&root), label, "{root} isn't the test's disk");
        // System32's own programs, by their path: an administrator's search
        // would look in this test's folder first.
        let system = mdif_core::os::sys::system_directory();
        let out = Command::new(system.join("fsutil.exe"))
            .args(["usn", "createjournal", "m=33554432", "a=4194304"])
            .arg(format!("{letter}:"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "fsutil: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        // `mountvol` and not a second DiskPart script, which wants a pause after
        // the first.
        let out = Command::new(system.join("mountvol.exe"))
            .arg(format!("{letter}:"))
            .arg("/D")
            .output()
            .unwrap();
        assert!(out.status.success(), "mountvol: {out:?}");
        disk
    }

    /// Runs `script` with DiskPart (System32's), 15 s after the last one ended,
    /// as DiskPart asks, from a new file in the folder only administrators may
    /// change.
    fn diskpart(&mut self, script: &str) {
        if let Some(last) = self.last {
            std::thread::sleep(Duration::from_secs(15).saturating_sub(last.elapsed()));
        }
        self.runs += 1;
        let file = self.admin.0.join(format!("script-{}.txt", self.runs));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)
            .and_then(|mut f| f.write_all(script.as_bytes()))
            .unwrap();
        let out = Command::new(mdif_core::os::sys::system_directory().join("diskpart.exe"))
            .arg("/s")
            .arg(&file)
            .output();
        self.last = Some(Instant::now());
        let _ = fs::remove_file(&file);
        let out = out.unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        eprintln!("{text}");
        // Detaching what is already detached is what was wanted.
        if text.contains("is already detached") {
            return;
        }
        assert!(
            out.status.success() && !text.contains("DiskPart has encountered an error"),
            "diskpart failed ({}): {text} {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Detaches it, three times at most: a volume that was just in use may take
    /// a moment to let go. The last failure, if none worked.
    fn try_detach(&mut self) -> std::thread::Result<()> {
        let script = format!(
            "select vdisk file=\"{}\"\ndetach vdisk\n",
            self.file.display()
        );
        let mut tried = Ok(());
        for _ in 0..3 {
            tried =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.diskpart(&script)));
            if tried.is_ok() {
                break;
            }
        }
        tried
    }

    /// Detaches it, and says so if that fails (a test that passed shouldn't leave
    /// a disk attached).
    fn detach(mut self) {
        self.attached = false;
        if let Err(e) = self.try_detach() {
            std::panic::resume_unwind(e);
        }
    }
}

impl Drop for Disk {
    fn drop(&mut self) {
        // Only while unwinding from a failure: as hard, but the failure already
        // says what went wrong. The folder goes after (a field).
        if self.attached {
            let _ = self.try_detach();
        }
    }
}

/// A new folder in Windows' own temporary one, named at random and made at
/// once to let only administrators and the system change it (its owner, if
/// that's this user, may only read): DiskPart, an administrator, runs scripts
/// from it, makes the disk's file in it and mounts the volume in it, and no
/// program of this user's that isn't elevated can change those, nor rename the
/// folder, nor find it (Windows' temporary folder lets users make things in it,
/// not list, rename or delete what others made). mdif-windows' bin test has the
/// same. Removed with what's in it when dropped.
struct AdminOnly(PathBuf);

impl AdminOnly {
    fn new() -> AdminOnly {
        use std::ffi::{OsStr, c_void};
        use std::os::windows::ffi::OsStrExt;
        #[repr(C)]
        struct SecurityAttributes {
            length: u32,
            descriptor: *mut c_void,
            inherit_handle: i32,
        }
        #[link(name = "advapi32")]
        unsafe extern "system" {
            fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl: *const u16,
                revision: u32,
                descriptor: *mut *mut c_void,
                size: *mut u32,
            ) -> i32;
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CreateDirectoryW(path: *const u16, attributes: *const SecurityAttributes) -> i32;
            fn LocalFree(memory: *mut c_void) -> *mut c_void;
        }
        let wide = |s: &OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
        // Owned by the administrators, nothing inherited: they and the system
        // may do anything, the owner (OW) only read.
        let sddl = wide(OsStr::new(
            "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FR;;;OW)",
        ));
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: `sddl` is NUL-ended; the descriptor is freed below.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1, // SDDL_REVISION_1
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
        let attributes = SecurityAttributes {
            length: std::mem::size_of::<SecurityAttributes>() as u32,
            descriptor,
            inherit_handle: 0,
        };
        let path = mdif_core::os::sys::system_directory()
            .with_file_name("Temp")
            .join(format!("mdif-{:016x}", random()));
        // SAFETY: the path is NUL-ended; `attributes` and the descriptor it
        // points to live across the call.
        let made = unsafe { CreateDirectoryW(wide(path.as_os_str()).as_ptr(), &attributes) };
        let error = std::io::Error::last_os_error();
        // SAFETY: made by the conversion above, freed once.
        unsafe { LocalFree(descriptor) };
        assert_ne!(made, 0, "{}: {error}", path.display());
        AdminOnly(path)
    }
}

impl Drop for AdminOnly {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A number nobody else can tell: the keys the system makes at random for each
/// hasher.
fn random() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

/// The label of the volume `root` (`Z:\`) names now.
fn label_of(root: &str) -> String {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetVolumeInformationW(
            root: *const u16,
            label: *mut u16,
            label_len: u32,
            serial: *mut u32,
            max_component: *mut u32,
            flags: *mut u32,
            file_system: *mut u16,
            file_system_len: u32,
        ) -> i32;
    }
    let path: Vec<u16> = std::ffi::OsStr::new(root)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut label = [0u16; 64];
    // SAFETY: `path` is NUL-ended and `label` the length passed; the outputs not
    // wanted are left out.
    let ok = unsafe {
        GetVolumeInformationW(
            path.as_ptr(),
            label.as_mut_ptr(),
            label.len() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    };
    assert_ne!(ok, 0, "{root}: {}", std::io::Error::last_os_error());
    let n = label.iter().position(|&c| c == 0).unwrap_or(label.len());
    String::from_utf16_lossy(&label[..n])
}

/// The letters in use, from Windows's list of logical drives (which doesn't depend
/// on whether a drive has media in it).
fn logical_drives() -> Vec<char> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLogicalDrives() -> u32;
    }
    // SAFETY: a plain call with no arguments.
    let mask = unsafe { GetLogicalDrives() };
    // Zero is the call failing (a PC always has a system drive), not no drives.
    assert_ne!(mask, 0, "{}", std::io::Error::last_os_error());
    ('A'..='Z')
        .filter(|l| mask & (1 << (*l as u8 - b'A')) != 0)
        .collect()
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
    let disk = Disk::attach();
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
        .with_owners(
            Rules::default().depth(mdif_core::os::fs::normalized(&vol), 1),
            PERIOD,
        )
        .unwrap()
        .with_detect(detect());
    // The folders are spelled as the journal and the lookups spell them, which for
    // a volume mounted in a folder is through that folder.
    let root = mdif_core::os::fs::normalized(&vol);
    let owner = |d: &str| root.join(d).display().to_string();

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
    // The store is this test's alone and watches this volume alone: every episode
    // in it is about the filler, and there is one.
    let ours: Vec<(&str, String)> = episodes
        .iter()
        .map(|e| (e.incident.kind.as_str(), e.incident.subject.clone()))
        .collect();
    assert_eq!(
        ours,
        [(Kind::Growth.as_str(), owner("fill"))],
        "{episodes:?}"
    );
    drop(w);
    disk.detach();
}
