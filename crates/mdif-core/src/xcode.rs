//! Xcode's leftovers, cleaned when asked (`mdif clean`, the app's Clean Up):
//!
//! - DerivedData, and the device support files Xcode copies from a device
//!   (`iOS DeviceSupport`), emptied while Xcode is closed: it makes them
//!   again, DerivedData at the next build ([`clean::Step::Empty`]).
//! - A simulator runtime, deleted with `simctl runtime delete`: Xcode
//!   downloads it again when it's needed.
//! - A simulator, erased with `simctl erase`: it stays in Xcode, as new; the
//!   apps installed in it and their data go. One whose runtime is gone is
//!   deleted (`simctl delete`), as `simctl delete unavailable` would.
//!
//! Read from `simctl`, run straight from the Xcode that's selected
//! ([`simctl`]): a fraction of a second, only when asked or when the disk
//! is measured.
//!
//! [`clean::Step::Empty`]: crate::clean::Step::Empty

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use serde::Serialize;

use crate::clean::Step;
use crate::space::{parse_time, run_within, simctl};
use crate::text::printable;
use crate::tidy::{is_uuid, parse_runtimes};

/// Xcode's bundle id: what has to be closed before its folders are emptied.
pub const XCODE: &str = "com.apple.dt.Xcode";
/// From the home folder.
pub const DERIVED_DATA: &str = "Library/Developer/Xcode/DerivedData";
/// The simulators' folders, from the home folder: one per simulator, named
/// by its UDID.
pub const DEVICES: &str = "Library/Developer/CoreSimulator/Devices";
/// Where simulator runtimes are kept: an asset folder each, in one of
/// these per platform.
const ASSETS: &str = "/System/Library/AssetsV2";
/// Simulators and runtimes nothing has used for this long are ticked in
/// Clean Up.
pub const UNUSED: i64 = 30 * 86_400;
/// A simulator holding less than this isn't worth erasing.
pub const SIMULATOR_MIN: u64 = 100_000_000;

/// Whether `dir` is what Xcode makes again, emptied while it's closed:
/// DerivedData, or a platform's device support files.
pub fn rebuilt(dir: &Path, home: &Path) -> bool {
    let Ok(rel) = dir.strip_prefix(home) else {
        return false;
    };
    let c: Option<Vec<&str>> = rel.iter().map(|n| n.to_str()).collect();
    matches!(
        c.as_deref(),
        Some(["Library", "Developer", "Xcode", x])
            if *x == "DerivedData" || x.ends_with("DeviceSupport")
    )
}

/// Whether `path` is a simulator runtime's asset folder:
/// `/System/Library/AssetsV2/com_apple_MobileAsset_iOSSimulatorRuntime/<hash>.asset`.
pub fn is_runtime(path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(ASSETS) else {
        return false;
    };
    let c: Option<Vec<&str>> = rel.iter().map(|n| n.to_str()).collect();
    matches!(
        c.as_deref(),
        Some([platform, asset])
            if platform.starts_with("com_apple_MobileAsset_")
                && platform.ends_with("SimulatorRuntime")
                && asset.ends_with(".asset")
    )
}

/// When a runtime's asset folder was made: when it was downloaded.
pub fn added(path: &Path) -> Option<i64> {
    let m = std::fs::symlink_metadata(path).ok()?;
    let t = m.created().or_else(|_| m.modified()).ok()?;
    t.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// Whether a runtime last used at `last_used` (never: `None`), downloaded
/// at `added`, has gone unused for [`UNUSED`]: one just downloaded and not
/// used yet hasn't.
pub fn runtime_unused(last_used: Option<i64>, added: Option<i64>, now: i64) -> bool {
    match (last_used, added) {
        (Some(used), _) => now - used >= UNUSED,
        (None, Some(added)) => now - added >= UNUSED,
        (None, None) => false,
    }
}

/// A simulator, as `simctl list devices -j` lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Simulator {
    pub udid: String,
    pub name: String,
    /// "iOS 27.0", from the runtime it's listed under.
    pub runtime: String,
    /// Its folder: what erasing it empties, and deleting it deletes.
    pub dir: Option<PathBuf>,
    /// What its data takes, as simctl counts it.
    pub bytes: u64,
    pub last_used: Option<i64>,
    pub booted: bool,
    /// Its runtime is installed.
    pub available: bool,
}

impl Simulator {
    /// Shut down, holding [`SIMULATOR_MIN`] or more, and not used for
    /// [`UNUSED`] (or ever): erasing it loses nothing used lately.
    pub fn unused(&self, now: i64) -> bool {
        self.available
            && !self.booted
            && self.bytes >= SIMULATOR_MIN
            && self.last_used.is_none_or(|t| now - t >= UNUSED)
    }

    /// "iPhone 17 (iOS 26.0)".
    pub fn title(&self) -> String {
        let name = printable(&self.name);
        if self.runtime.is_empty() {
            name.into_owned()
        } else {
            format!("{name} ({})", printable(&self.runtime))
        }
    }
}

/// Every simulator, from the Xcode that's selected; none without Xcode.
pub fn simulators() -> Vec<Simulator> {
    let Some(simctl) = simctl() else {
        return Vec::new();
    };
    let mut c = Command::new(simctl);
    c.args(["list", "devices", "-j"]);
    run_within(c, Duration::from_secs(30))
        .filter(|out| out.status.success())
        .map(|out| parse_simulators(&out.stdout))
        .unwrap_or_default()
}

/// `simctl list devices -j`.
pub fn parse_simulators(out: &[u8]) -> Vec<Simulator> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(out) else {
        return Vec::new();
    };
    let Some(by_runtime) = v["devices"].as_object() else {
        return Vec::new();
    };
    let mut found: Vec<Simulator> = by_runtime
        .iter()
        .filter_map(|(runtime, list)| Some((runtime_label(runtime), list.as_array()?)))
        .flat_map(|(runtime, list)| {
            list.iter().filter_map(move |d| {
                let available = d["isAvailable"].as_bool().unwrap_or(true)
                    // Before Xcode 10.1: "(unavailable, runtime profile not found)".
                    && !d["availability"]
                        .as_str()
                        .is_some_and(|a| a.contains("unavailable"));
                Some(Simulator {
                    udid: d["udid"].as_str()?.to_string(),
                    name: d["name"].as_str().unwrap_or("").to_string(),
                    runtime: runtime.clone(),
                    dir: d["dataPath"]
                        .as_str()
                        .and_then(|p| Path::new(p).parent())
                        .map(Path::to_path_buf),
                    bytes: d["dataPathSize"].as_u64().unwrap_or(0),
                    last_used: d["lastUsedAt"]
                        .as_str()
                        .or(d["lastBootedAt"].as_str())
                        .and_then(parse_time),
                    booted: d["state"].as_str() == Some("Booted"),
                    available,
                })
            })
        })
        .collect();
    found.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.udid.cmp(&b.udid)));
    found
}

/// "iOS 27.0", for `com.apple.CoreSimulator.SimRuntime.iOS-27-0` (or, from
/// before Xcode 10.1, `iOS 12.1`).
fn runtime_label(key: &str) -> String {
    let Some(last) = key.strip_prefix("com.apple.CoreSimulator.SimRuntime.") else {
        return key.to_string();
    };
    match last.split_once('-') {
        Some((platform, version)) => format!("{platform} {}", version.replace('-', ".")),
        None => last.to_string(),
    }
}

/// What cleaning `path` means, when it's one of Xcode's: `None` when it
/// isn't. DerivedData and device support go as caches do
/// ([`clean::plan`](crate::clean::plan) empties them); this is for
/// runtimes and simulators.
pub fn plan(path: &Path, home: &Path) -> Option<Result<Step, String>> {
    if is_runtime(path) {
        return Some(runtime_step(path));
    }
    let udid = path
        .strip_prefix(home.join(DEVICES))
        .ok()
        .and_then(|rel| rel.to_str())
        .filter(|rel| is_uuid(rel))?;
    Some(simulator_step(path, udid))
}

fn runtime_step(path: &Path) -> Result<Step, String> {
    let shown = path.display();
    let simctl = simctl().ok_or("Deleting a simulator runtime needs Xcode.")?;
    let mut c = Command::new(simctl);
    c.args(["runtime", "list", "-j"]);
    let out = run_within(c, Duration::from_secs(30))
        .filter(|out| out.status.success())
        .ok_or("simctl didn't list the runtimes.")?;
    let rt = parse_runtimes(&out.stdout)
        .into_iter()
        .find(|r| r.path == path)
        .ok_or_else(|| format!("{shown} isn't a simulator runtime Xcode has now."))?;
    if !is_uuid(&rt.identifier) {
        return Err(format!("{}: simctl gave no id for it.", rt.name));
    }
    Ok(Step::DeleteRuntime {
        path: path.to_path_buf(),
        uuid: rt.identifier,
        name: rt.name,
    })
}

fn simulator_step(dir: &Path, udid: &str) -> Result<Step, String> {
    let s = simulators()
        .into_iter()
        .find(|s| s.udid == udid)
        .ok_or_else(|| format!("No simulator has the folder {} now.", dir.display()))?;
    if s.booted {
        return Err(format!("{} is running: shut it down first.", s.title()));
    }
    Ok(Step::Simulator {
        dir: dir.to_path_buf(),
        udid: s.udid.clone(),
        name: s.title(),
        erase: s.available,
    })
}

/// Deletes the runtime `uuid` (`simctl runtime delete`).
pub fn delete_runtime(uuid: &str, name: &str) -> Result<(), String> {
    let args = crate::tidy::runtime_delete_args(uuid).ok_or("Not a runtime's id.")?;
    let out = sim(&args, 600)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("The {name} wasn't deleted: {}", said(&out)))
    }
}

/// Erases the simulator `udid` (`simctl erase`), or deletes it (`erase`
/// false), if it's still shut down.
pub fn reset(udid: &str, name: &str, erase: bool) -> Result<(), String> {
    if simulators().iter().any(|s| s.udid == udid && s.booted) {
        return Err(format!("{name} is running: shut it down first."));
    }
    let args = if erase {
        erase_args(udid)
    } else {
        crate::tidy::device_delete_args(udid)
    }
    .ok_or("Not a simulator's id.")?;
    let out = sim(&args, 120)?;
    if out.status.success() {
        Ok(())
    } else {
        let verb = if erase { "erased" } else { "deleted" };
        Err(format!("{name} wasn't {verb}: {}", said(&out)))
    }
}

/// `simctl erase <udid>`; nothing for what isn't a UUID (simctl takes `all`
/// too).
pub fn erase_args(udid: &str) -> Option<Vec<String>> {
    is_uuid(udid).then(|| vec!["erase".to_string(), udid.to_string()])
}

fn sim(args: &[String], wait: u64) -> Result<Output, String> {
    let simctl = simctl().ok_or("Needs Xcode, which isn't installed.")?;
    let mut c = Command::new(simctl);
    c.args(args);
    run_within(c, Duration::from_secs(wait)).ok_or_else(|| "simctl didn't answer in time.".into())
}

/// What simctl said, its first line.
fn said(out: &Output) -> String {
    let text = String::from_utf8_lossy(if out.stderr.is_empty() {
        &out.stdout
    } else {
        &out.stderr
    })
    .trim()
    .lines()
    .next()
    .unwrap_or("")
    .to_string();
    format!("simctl: {}", printable(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    #[test]
    fn xcode_folders_it_makes_again_are_known() {
        let home = Path::new("/Users/me");
        for yes in [
            "/Users/me/Library/Developer/Xcode/DerivedData",
            "/Users/me/Library/Developer/Xcode/iOS DeviceSupport",
            "/Users/me/Library/Developer/Xcode/watchOS DeviceSupport",
        ] {
            assert!(rebuilt(Path::new(yes), home), "{yes}");
        }
        for no in [
            "/Users/me/Library/Developer/Xcode",
            "/Users/me/Library/Developer/Xcode/Archives",
            "/Users/me/Library/Developer/Xcode/DerivedData/App-abc",
            "/Users/other/Library/Developer/Xcode/DerivedData",
        ] {
            assert!(!rebuilt(Path::new(no), home), "{no}");
        }
    }

    #[test]
    fn a_runtime_is_its_asset_folder() {
        assert!(is_runtime(Path::new(
            "/System/Library/AssetsV2/com_apple_MobileAsset_iOSSimulatorRuntime/3e80.asset"
        )));
        for no in [
            "/System/Library/AssetsV2/com_apple_MobileAsset_iOSSimulatorRuntime",
            "/System/Library/AssetsV2/com_apple_MobileAsset_Font7/3e80.asset",
            "/System/Library/AssetsV2/com_apple_MobileAsset_iOSSimulatorRuntime/3e80.asset/AssetData",
            "/Users/me/System/Library/AssetsV2/com_apple_MobileAsset_iOSSimulatorRuntime/a.asset",
        ] {
            assert!(!is_runtime(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn a_runtime_just_downloaded_isnt_unused() {
        let now = 1_800_000_000;
        assert!(runtime_unused(Some(now - 31 * DAY), None, now));
        assert!(!runtime_unused(
            Some(now - 3 * DAY),
            Some(now - 400 * DAY),
            now
        ));
        // Never used: by when it came.
        assert!(runtime_unused(None, Some(now - 40 * DAY), now));
        assert!(!runtime_unused(None, Some(now - 10 * DAY), now));
        assert!(!runtime_unused(None, None, now));
    }

    #[test]
    fn simulators_are_read_with_their_runtime_and_last_use() {
        let out = br#"{
          "devices" : {
            "com.apple.CoreSimulator.SimRuntime.iOS-27-0" : [
              {
                "lastUsedAt" : "2026-10-05T19:15:01Z",
                "dataPath" : "/Users/me/Library/Developer/CoreSimulator/Devices/2ED1738B-87FE-411D-9108-82FB03264E5A/data",
                "dataPathSize" : 2555953152,
                "udid" : "2ED1738B-87FE-411D-9108-82FB03264E5A",
                "isAvailable" : true,
                "state" : "Shutdown",
                "name" : "iPhone 17"
              },
              {
                "dataPath" : "/Users/me/Library/Developer/CoreSimulator/Devices/BFE8B5A4-FC91-4D33-9148-9755D340E107/data",
                "dataPathSize" : 4964089856,
                "udid" : "BFE8B5A4-FC91-4D33-9148-9755D340E107",
                "isAvailable" : true,
                "state" : "Booted",
                "name" : "iPhone 18 Pro"
              }
            ],
            "com.apple.CoreSimulator.SimRuntime.watchOS-11-0" : [
              {
                "udid" : "11111111-2222-3333-4444-555555555555",
                "isAvailable" : false,
                "state" : "Shutdown",
                "name" : "Apple Watch"
              }
            ]
          }
        }"#;
        let s = parse_simulators(out);
        let titles: Vec<String> = s.iter().map(Simulator::title).collect();
        assert_eq!(
            titles,
            [
                "iPhone 18 Pro (iOS 27.0)",
                "iPhone 17 (iOS 27.0)",
                "Apple Watch (watchOS 11.0)"
            ]
        );
        assert!(s[0].booted && s[0].last_used.is_none());
        assert_eq!(s[1].last_used, parse_time("2026-10-05T19:15:01Z"));
        assert_eq!(
            s[1].dir.as_deref(),
            Some(Path::new(
                "/Users/me/Library/Developer/CoreSimulator/Devices/2ED1738B-87FE-411D-9108-82FB03264E5A"
            ))
        );
        assert!(!s[2].available);
        assert!(parse_simulators(b"{}").is_empty());
        assert!(parse_simulators(b"not json").is_empty());

        // Unused: shut down, with something in it, not used for 30 days.
        let at = |days| parse_time("2026-10-05T19:15:01Z").unwrap() + days * DAY;
        assert!(s[1].unused(at(30)));
        assert!(!s[1].unused(at(29)));
        // Running, or empty, or without its runtime: not this.
        assert!(!s[0].unused(at(400)));
        assert!(!s[2].unused(at(400)));
    }

    #[test]
    fn simulators_are_named_only_by_their_folder() {
        let home = Path::new("/Users/me");
        let devices = home.join(DEVICES);
        // Not a simulator's folder: none of Xcode's.
        assert!(plan(&devices, home).is_none());
        assert!(plan(&devices.join("device_set.plist"), home).is_none());
        assert!(plan(&devices.join("all"), home).is_none());
        assert!(
            plan(
                &devices.join("2ED1738B-87FE-411D-9108-82FB03264E5A/data"),
                home
            )
            .is_none()
        );
        assert_eq!(
            erase_args("2ED1738B-87FE-411D-9108-82FB03264E5A").unwrap(),
            ["erase", "2ED1738B-87FE-411D-9108-82FB03264E5A"]
        );
        assert_eq!(erase_args("all"), None);
    }
}
