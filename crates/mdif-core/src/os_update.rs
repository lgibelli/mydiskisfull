//! A macOS update downloaded and prepared, waiting for the restart that
//! installs it. Until then the Mac keeps macOS's cryptexes twice in Preboot
//! (`cryptex1/current` and the update's `cryptex1/proposed`), the update's
//! staging on the Update volume and its download in AssetsV2: tens of GB,
//! taken in a few minutes, that come back once it's installed. All of it is
//! readable without root.

use std::path::{Path, PathBuf};

/// What softwareupdated wrote when it prepared the update: the update's
/// asset attributes (`OSVersion`, `Build`, sizes).
const BRAIN: &str = "/System/Volumes/Update/brain_path.plist";
const RUNNING: &str = "/System/Library/CoreServices/SystemVersion.plist";
const PREBOOT: &str = "/System/Volumes/Preboot";
const UPDATE: &str = "/System/Volumes/Update";
const DOWNLOAD: &str = "/System/Library/AssetsV2/com_apple_MobileAsset_MacSoftwareUpdate";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    /// "27.2".
    pub version: String,
    /// What the restart that installs it frees: the cryptexes it replaces,
    /// the Update volume and the download.
    pub bytes: u64,
}

/// The update prepared on this Mac, if there is one: a version other than
/// the one running, with its cryptexes already in Preboot.
pub fn staged() -> Option<Staged> {
    let (target, running, cryptex) = prepared()?;
    // What only the running macOS's cryptexes hold: they share nothing with
    // the update's, and go when it's installed.
    let outgoing = real(&cryptex)?.saturating_sub(real(&cryptex.join("proposed"))?);
    let staging = real(Path::new(UPDATE)).unwrap_or(0) + real(Path::new(DOWNLOAD)).unwrap_or(0);
    waiting(&target, &running, outgoing + staging)
}

/// When that update was prepared (its cryptexes arrived in Preboot), as
/// Unix seconds: a few `stat`s, cheap enough for every forecast.
pub fn prepared_at() -> Option<i64> {
    let (target, running, cryptex) = prepared()?;
    waiting(&target, &running, 1)?;
    let created = std::fs::metadata(cryptex.join("proposed"))
        .ok()?
        .created()
        .ok()?;
    let secs = created
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(secs as i64)
}

/// The update's version, the running one, and the `cryptex1` folder that
/// holds the update's cryptexes beside the running ones.
fn prepared() -> Option<(String, String, PathBuf)> {
    let target = version(Path::new(BRAIN), "OSVersion")?;
    let running = version(Path::new(RUNNING), "ProductVersion")?;
    let cryptex = std::fs::read_dir(PREBOOT)
        .ok()?
        .flatten()
        .map(|e| e.path().join("cryptex1"))
        .find(|c| c.join("proposed").is_dir())?;
    Some((target, running, cryptex))
}

/// An update is waiting when it isn't the macOS running: the plist stays
/// behind once it's installed.
fn waiting(target: &str, running: &str, bytes: u64) -> Option<Staged> {
    (!target.is_empty() && target != running && bytes > 0).then(|| Staged {
        version: target.to_string(),
        bytes,
    })
}

fn version(plist: &Path, key: &str) -> Option<String> {
    let v = plist::Value::from_file(plist).ok()?;
    Some(v.as_dictionary()?.get(key)?.as_string()?.to_string())
}

/// Every block once, however many clones share it (the cryptexes are
/// cloned within each set).
fn real(path: &Path) -> Option<u64> {
    let opts = crate::scan::Options {
        max_depth: 0,
        min_node: u64::MAX,
        big_files: None,
    };
    crate::scan::scan(path, &opts).ok().map(|s| s.real)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_only_for_another_version() {
        assert_eq!(
            waiting("27.2", "27.0.1", 13),
            Some(Staged {
                version: "27.2".into(),
                bytes: 13
            })
        );
        // Installed: the plist is left behind.
        assert_eq!(waiting("27.2", "27.2", 13), None);
        assert_eq!(waiting("", "27.2", 13), None);
        assert_eq!(waiting("27.2", "27.0.1", 0), None);
    }

    #[test]
    fn versions_are_read_from_a_plist() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("brain_path.plist");
        let mut d = plist::Dictionary::new();
        d.insert("OSVersion".into(), "27.2".into());
        d.insert("Build".into(), "26B123".into());
        plist::Value::Dictionary(d).to_file_xml(&p).unwrap();
        assert_eq!(version(&p, "OSVersion").as_deref(), Some("27.2"));
        assert_eq!(version(&p, "ProductVersion"), None);
        assert_eq!(version(&dir.path().join("none.plist"), "OSVersion"), None);
    }
}
