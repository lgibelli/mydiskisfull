//! The whole-disk map: what APFS says is in use, what we can measure, and
//! the difference, so no gigabyte goes unexplained.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mdif_macos::fs::{self, FsStats};
use serde::Serialize;

use crate::apfs::{self, Container, Volume};
use crate::scan::{self, Options, Scan};

/// Where the boot Data volume is mounted; `/Users`, `/Applications`,
/// `/Library`, `/private`, `/opt` ... are firmlinks into it.
pub const DATA_ROOT: &str = "/System/Volumes/Data";

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
    let (container, volume) = apfs::find_volume(apfs::containers()?, &st.device)?;
    let snapshots = apfs::local_snapshots(Path::new(&st.mount_point)).unwrap_or_default();
    let scan = scan::scan(root, opts).with_context(|| format!("scanning {}", root.display()))?;
    Ok(DiskMap {
        root: root.to_path_buf(),
        container,
        volume,
        fs: st,
        snapshots,
        scan,
    })
}
