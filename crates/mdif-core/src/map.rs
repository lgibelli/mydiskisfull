//! The whole-disk map: what APFS says is in use, what we can measure, and
//! the difference, so no gigabyte goes unexplained.

use std::path::{Path, PathBuf};

use crate::os::fs::{self, FsStats};
use anyhow::{Context, Result};
use serde::Serialize;

#[cfg(target_os = "macos")]
use crate::apfs;
use crate::apfs::{Container, Volume};
use crate::scan::{self, Options, Scan};

/// Where macOS mounts the boot Data volume; `/Users`, `/Applications`,
/// `/Library`, `/private`, `/opt` ... are firmlinks into it. Paths are shown
/// without it. No path of another OS starts with it, so stripping it from
/// one there changes nothing.
pub const DATA_ROOT: &str = "/System/Volumes/Data";

/// The boot volume, where the user's files are: the Data volume on macOS, the
/// system drive on Windows.
pub fn boot_volume() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(DATA_ROOT)
    }
    #[cfg(windows)]
    {
        // The drive Windows is installed on, whichever letter it has.
        let system = crate::os::sys::system_directory();
        system
            .ancestors()
            .last()
            .map_or(system.clone(), Path::to_path_buf)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskMap {
    pub root: PathBuf,
    pub container: Container,
    /// The volume `root` lives on.
    pub volume: Volume,
    #[serde(skip)]
    pub fs: FsStats,
    pub snapshots: Vec<String>,
    pub scan: Scan,
    /// What the figures are not exact about, in words (Windows).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// The disk figures are the user's quota, not the volume's, so scanned bytes
    /// can't be reconciled with them.
    #[serde(skip)]
    pub quota_limited: bool,
}

impl DiskMap {
    /// Space the scanned tree really uses (clones and hard links counted
    /// once).
    pub fn measured(&self) -> u64 {
        self.scan.real
    }

    /// In use on the volume but not found by the scan: files only root can
    /// read, file-system metadata, and blocks held only by snapshots.
    /// Meaningful when `root` is the volume root.
    pub fn unmeasured(&self) -> i64 {
        self.volume.capacity_in_use as i64 - self.measured() as i64
    }

    /// `path` as the user knows it: `/System/Volumes/Data/Users/x` -> `/Users/x`.
    pub fn display_path(&self, path: &Path) -> String {
        match path.strip_prefix(DATA_ROOT) {
            Ok(rest) if rest.as_os_str().is_empty() => "/".into(),
            Ok(rest) => format!("/{}", rest.display()),
            Err(_) => path.display().to_string(),
        }
    }
}

pub fn build(root: &Path, opts: &Options) -> Result<DiskMap> {
    fs::forbid_dataless_materialization().context("disabling cloud downloads for this process")?;
    let st = fs::statfs(root).with_context(|| format!("statfs {}", root.display()))?;
    #[cfg(target_os = "macos")]
    let (container, volume, snapshots) = {
        let (mut container, mut volume) = apfs::find_volume(apfs::containers()?, &st.device)?;
        if volume.capacity_in_use == 0 {
            apfs::fill_missing_sizes(&mut container, &st);
            volume = apfs::find_volume(vec![container.clone()], &st.device)?.1;
        }
        let snapshots = apfs::local_snapshots(Path::new(&st.mount_point)).unwrap_or_default();
        (container, volume, snapshots)
    };
    // A Windows volume stands alone: its own size and what's free, and no
    // snapshots are listed yet (System Restore points come with the backups page).
    #[cfg(windows)]
    let (container, volume, snapshots) = {
        let volume = Volume {
            name: st.mount_point.clone(),
            device_identifier: st.device.clone(),
            roles: vec![format!("Windows {}", st.mount_point.trim_end_matches('\\'))],
            capacity_in_use: st.total.saturating_sub(st.avail),
        };
        let container = Container {
            container_reference: st.mount_point.clone(),
            capacity_ceiling: st.total,
            capacity_free: st.avail,
            volumes: vec![volume.clone()],
        };
        (container, volume, Vec::new())
    };
    #[cfg(windows)]
    let (notes, quota_limited) = {
        // ReFS keeps 128-bit file IDs and shares blocks between files: neither is
        // measured, so what it says about hard links and clones would be wrong.
        let kind = fs::file_system(root).unwrap_or_default();
        if kind.eq_ignore_ascii_case("ReFS") {
            anyhow::bail!(
                "{} is ReFS (a Dev Drive), whose shared blocks and file numbers aren't read yet",
                st.mount_point
            );
        }
        let quota = fs::quota_limited(root).unwrap_or(false);
        let mut notes = vec![
            "A file under 1 MiB is taken to have one name, so a small hard-linked file is counted \
             once per name here and its share can be off."
                .to_string(),
            "A file held open for writing can be listed behind what it really takes; the next scan \
             after it closes shows it."
                .to_string(),
        ];
        if quota {
            notes.push(
                "This account has a disk quota, so the disk's size and free space are its quota's, \
                 and what the scan finds isn't compared with them."
                    .to_string(),
            );
        }
        (notes, quota)
    };
    #[cfg(not(windows))]
    let (notes, quota_limited) = (Vec::new(), false);
    let scan = scan::scan(root, opts).with_context(|| format!("scanning {}", root.display()))?;
    Ok(DiskMap {
        root: root.to_path_buf(),
        container,
        volume,
        fs: st,
        snapshots,
        scan,
        notes,
        quota_limited,
    })
}
