//! APFS containers and volumes (`diskutil apfs list -plist`) and local Time
//! Machine snapshots (`tmutil listlocalsnapshots`). Neither needs root.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use mdif_macos::fs::{self, FsStats};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Container {
    pub container_reference: String,
    // In the App Sandbox `diskutil` leaves the sizes out: see
    // `fill_missing_sizes`.
    #[serde(default)]
    pub capacity_ceiling: u64,
    #[serde(default)]
    pub capacity_free: u64,
    pub volumes: Vec<Volume>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Volume {
    pub name: String,
    pub device_identifier: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub capacity_in_use: u64,
}

impl Volume {
    /// "Data", "System", "VM", ...; the name when a volume has no role.
    pub fn label(&self) -> &str {
        self.roles
            .first()
            .map_or(self.name.as_str(), String::as_str)
    }
}

impl Container {
    pub fn used(&self) -> u64 {
        self.capacity_ceiling.saturating_sub(self.capacity_free)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ApfsList {
    containers: Vec<Container>,
}

pub fn containers() -> Result<Vec<Container>> {
    let out = Command::new("diskutil")
        .args(["apfs", "list", "-plist"])
        .output()
        .context("running diskutil")?;
    ensure!(
        out.status.success(),
        "diskutil apfs list failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(plist::from_bytes::<ApfsList>(&out.stdout)
        .context("parsing diskutil output")?
        .containers)
}

/// The container holding the volume mounted from `device` (e.g.
/// `/dev/disk3s5`), and that volume.
pub fn find_volume(containers: Vec<Container>, device: &str) -> Result<(Container, Volume)> {
    let id = device.trim_start_matches("/dev/");
    for c in containers {
        if let Some(v) = c
            .volumes
            .iter()
            .find(|v| v.device_identifier == id)
            .cloned()
        {
            return Ok((c, v));
        }
    }
    bail!("no APFS volume {id}")
}

/// In the App Sandbox `diskutil` lists the containers and their volumes but
/// not their sizes. Take the container's from `statfs` of a volume in it
/// (`st`), and each mounted volume's from the volume itself; one that isn't
/// mounted (Recovery) stays at 0.
pub fn fill_missing_sizes(c: &mut Container, st: &FsStats) {
    if c.capacity_ceiling == 0 {
        c.capacity_ceiling = st.total;
        c.capacity_free = st.free;
    }
    let mounts = fs::mounts().unwrap_or_default();
    for v in c.volumes.iter_mut().filter(|v| v.capacity_in_use == 0) {
        if let Some(m) = mount_of(&mounts, &v.device_identifier) {
            v.capacity_in_use = fs::space_used(Path::new(m)).unwrap_or(0);
        }
    }
}

/// Where the volume `id` (`disk3s1`) is mounted: from itself, or from one of
/// its snapshots (the system volume boots from `disk3s1s1`).
fn mount_of<'a>(mounts: &'a [(String, String)], id: &str) -> Option<&'a str> {
    let of = |want: fn(&str) -> bool| {
        mounts.iter().find_map(|(dev, m)| {
            let rest = dev.strip_prefix("/dev/")?.strip_prefix(id)?;
            want(rest).then_some(m.as_str())
        })
    };
    of(str::is_empty).or_else(|| {
        of(|r| {
            r.strip_prefix('s')
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
    })
}

/// Names of local Time Machine snapshots on the volume mounted at `mount`.
/// They keep deleted files' blocks allocated until they expire.
///
/// Not the snapshots macOS keeps while it installs an update
/// (`com.apple.os.update-…`): they aren't Time Machine's to delete, and go
/// with the update. Nor one listed as `(dataless)`: its blocks are released
/// already, and it holds nothing.
pub fn local_snapshots(mount: &Path) -> Result<Vec<String>> {
    let out = Command::new("tmutil")
        .arg("listlocalsnapshots")
        .arg(mount)
        .output()
        .context("running tmutil")?;
    ensure!(
        out.status.success(),
        "tmutil listlocalsnapshots failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(parse_snapshots(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_snapshots(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("com.apple.TimeMachine.") && !l.ends_with("(dataless)"))
        .filter_map(|l| l.split_whitespace().next())
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_volume_mounted_from_itself_or_a_snapshot() {
        let mounts = [
            ("/dev/disk3s1s1", "/"),
            ("/dev/disk3s5", "/System/Volumes/Data"),
            ("/dev/disk3s10", "/Volumes/Other"),
        ]
        .map(|(d, m)| (d.to_string(), m.to_string()));
        assert_eq!(mount_of(&mounts, "disk3s1"), Some("/"));
        assert_eq!(mount_of(&mounts, "disk3s5"), Some("/System/Volumes/Data"));
        assert_eq!(mount_of(&mounts, "disk3s10"), Some("/Volumes/Other"));
        assert_eq!(mount_of(&mounts, "disk3s3"), None);
    }

    #[test]
    fn sizes_left_out_are_filled_in() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>Containers</key><array><dict>
<key>ContainerReference</key><string>disk3</string>
<key>Volumes</key><array><dict><key>Name</key><string>Data</string>
<key>DeviceIdentifier</key><string>disk3s5</string><key>Roles</key><array><string>Data</string></array>
</dict></array></dict></array></dict></plist>"#;
        let mut c = plist::from_bytes::<ApfsList>(xml.as_bytes())
            .unwrap()
            .containers
            .remove(0);
        assert_eq!((c.capacity_ceiling, c.volumes[0].capacity_in_use), (0, 0));
        let st = fs::statfs(Path::new("/System/Volumes/Data")).unwrap();
        // This Mac's own Data volume stands in for the one in the listing.
        c.volumes[0].device_identifier = st.device.trim_start_matches("/dev/").into();
        fill_missing_sizes(&mut c, &st);
        assert_eq!(c.capacity_ceiling, st.total);
        assert!(c.volumes[0].capacity_in_use > 0);
    }

    #[test]
    fn parses_tmutil_output() {
        let out = "Snapshots for disk /System/Volumes/Data:\ncom.apple.TimeMachine.2026-10-16-191222.local\n";
        assert_eq!(
            parse_snapshots(out),
            ["com.apple.TimeMachine.2026-10-16-191222.local"]
        );
        assert!(parse_snapshots("Snapshots for disk /:\n").is_empty());
        // As on a Mac with an update waiting for a restart, after macOS
        // released the older snapshot's blocks to make room.
        let out = "Snapshots for volume group containing disk /System/Volumes/Data:
com.apple.TimeMachine.2026-10-26-030412.local
com.apple.TimeMachine.2026-10-26-090437.local (dataless)
com.apple.os.update-519C13F331E7E4D12810265CA118595DB1E847980966608AE2A7FA5C568764B63
com.apple.os.update-MSUPrepareUpdate
";
        assert_eq!(
            parse_snapshots(out),
            ["com.apple.TimeMachine.2026-10-26-030412.local"]
        );
    }

    #[test]
    fn finds_the_boot_data_volume() {
        let st = mdif_macos::fs::statfs(Path::new("/System/Volumes/Data")).unwrap();
        let (c, v) = find_volume(containers().unwrap(), &st.device).unwrap();
        assert_eq!(v.label(), "Data");
        assert!(c.capacity_ceiling > v.capacity_in_use);
        assert!(c.volumes.iter().any(|v| v.label() == "System"));
    }
}
