//! APFS containers and volumes (`diskutil apfs list -plist`) and local Time
//! Machine snapshots (`tmutil listlocalsnapshots`). Neither needs root.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Container {
    pub container_reference: String,
    pub capacity_ceiling: u64,
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

/// Names of local Time Machine snapshots on the volume mounted at `mount`.
/// They keep deleted files' blocks allocated until they expire.
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
        .filter(|l| l.starts_with("com.apple."))
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tmutil_output() {
        let out = "Snapshots for disk /System/Volumes/Data:\ncom.apple.TimeMachine.2026-10-16-191222.local\n";
        assert_eq!(
            parse_snapshots(out),
            ["com.apple.TimeMachine.2026-10-16-191222.local"]
        );
        assert!(parse_snapshots("Snapshots for disk /:\n").is_empty());
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
