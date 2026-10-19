//! Configuration profiles: how a tweak sets another app's *managed*
//! preferences, the kind it treats as policy (Chrome's, for one).
//!
//! macOS no longer installs profiles from the command line. We write the
//! `.mobileconfig`, open it, and the user approves it in System Settings
//! (Privacy & Security → Profiles). The profile is user-scoped and only
//! carries preferences for one app's domain, in the Custom Settings form
//! Apple Configurator writes. Once installed, its keys appear in
//! `/Library/Managed Preferences/<user>/<domain>.plist`, which is how we
//! tell it's in effect. Removing one of ours needs no password.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Where macOS puts preferences that profiles manage.
const MANAGED: &str = "/Library/Managed Preferences";

/// System Settings → Privacy & Security → Profiles.
pub const SETTINGS_PANE: &str = "x-apple.systempreferences:com.apple.Profiles-Settings.extension";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// Reverse-DNS, unique to the tweak; installing it again replaces it.
    pub identifier: String,
    /// What System Settings lists it as.
    pub name: String,
    pub description: String,
    /// The app's preferences domain, e.g. `com.google.Chrome`.
    pub domain: String,
    pub prefs: Map<String, Value>,
}

impl Profile {
    pub fn file_in(&self, dir: &Path) -> PathBuf {
        dir.join(format!("{}.mobileconfig", self.identifier))
    }

    /// The profile as an XML property list.
    pub fn mobileconfig(&self) -> Result<Vec<u8>> {
        let mut prefs = plist::Dictionary::new();
        for (k, v) in &self.prefs {
            prefs.insert(k.clone(), to_plist(v)?);
        }
        let forced = dict([(
            "Forced",
            plist::Value::Array(vec![
                dict([("mcx_preference_settings", plist::Value::Dictionary(prefs))]).into(),
            ]),
        )]);
        let payload = dict([
            ("PayloadType", "com.apple.ManagedClient.preferences".into()),
            (
                "PayloadIdentifier",
                format!("{}.prefs", self.identifier).into(),
            ),
            (
                "PayloadUUID",
                uuid(&format!("{}.prefs", self.identifier)).into(),
            ),
            ("PayloadVersion", 1.into()),
            ("PayloadDisplayName", self.domain.clone().into()),
            ("PayloadEnabled", true.into()),
            (
                "PayloadContent",
                dict([(self.domain.as_str(), forced.into())]).into(),
            ),
        ]);
        let top = dict([
            ("PayloadContent", plist::Value::Array(vec![payload.into()])),
            ("PayloadDisplayName", self.name.clone().into()),
            ("PayloadDescription", self.description.clone().into()),
            ("PayloadIdentifier", self.identifier.clone().into()),
            ("PayloadOrganization", "MyDiskIsFull".into()),
            ("PayloadScope", "User".into()),
            ("PayloadType", "Configuration".into()),
            ("PayloadUUID", uuid(&self.identifier).into()),
            ("PayloadVersion", 1.into()),
            ("PayloadRemovalDisallowed", false.into()),
        ]);
        let mut out = Vec::new();
        plist::Value::Dictionary(top).to_writer_xml(&mut out)?;
        Ok(out)
    }

    /// Whether every one of its preferences is in effect as a managed
    /// preference (for this user, or for the whole Mac).
    pub fn in_effect(&self) -> bool {
        let user = std::env::var("USER").unwrap_or_default();
        self.in_effect_under(Path::new(MANAGED), &user)
    }

    pub fn in_effect_under(&self, managed: &Path, user: &str) -> bool {
        let file = format!("{}.plist", self.domain);
        [managed.join(user).join(&file), managed.join(&file)]
            .iter()
            .any(|p| {
                plist::Value::from_file(p)
                    .ok()
                    .and_then(plist::Value::into_dictionary)
                    .is_some_and(|d| {
                        self.prefs.iter().all(|(k, v)| {
                            d.get(k).and_then(|x| to_plist(v).ok().map(|v| *x == v)) == Some(true)
                        })
                    })
            })
    }

    /// Saved in `dir` and handed to System Settings, but not installed yet.
    pub fn downloaded(&self, dir: &Path) -> bool {
        self.file_in(dir).exists() && !self.in_effect()
    }

    /// Save it in `dir`, open it so System Settings offers to install it,
    /// and open the pane where that happens. The user approves it there.
    pub fn download(&self, dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let file = self.file_in(dir);
        std::fs::write(&file, self.mobileconfig()?)
            .with_context(|| format!("writing {}", file.display()))?;
        for target in [file.as_os_str(), SETTINGS_PANE.as_ref()] {
            let st = Command::new("/usr/bin/open").arg(target).status()?;
            if !st.success() {
                bail!("open {} failed", target.to_string_lossy());
            }
        }
        Ok(file)
    }

    /// Remove it if installed (no password needed for our own user-scoped
    /// profile), and forget the downloaded file.
    pub fn remove(&self, dir: &Path) -> Result<()> {
        if self.in_effect() {
            let out = Command::new("/usr/bin/profiles")
                .args(["remove", "-identifier", &self.identifier])
                .output()
                .context("running profiles")?;
            if !out.status.success() && self.in_effect() {
                bail!(
                    "couldn't remove the profile \"{}\" ({}). Remove it in System Settings → \
                     Privacy & Security → Profiles.",
                    self.name,
                    String::from_utf8_lossy(&out.stdout).trim()
                );
            }
        }
        match std::fs::remove_file(self.file_in(dir)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

fn dict<'a>(entries: impl IntoIterator<Item = (&'a str, plist::Value)>) -> plist::Dictionary {
    entries
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

fn to_plist(v: &Value) -> Result<plist::Value> {
    Ok(match v {
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => i.into(),
            (None, Some(f)) => f.into(),
            _ => bail!("{n} doesn't fit a property list"),
        },
        Value::String(s) => s.clone().into(),
        v => bail!("only booleans, numbers and strings go in a profile, not {v}"),
    })
}

/// A UUID derived from `name`, so the same profile always has the same one.
fn uuid(name: &str) -> String {
    // FNV-1a, twice with different offsets, for 128 bits.
    let fnv = |offset: u64| {
        name.bytes().fold(offset, |h, b| {
            (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
        })
    };
    let (hi, lo) = (fnv(0xcbf2_9ce4_8422_2325), fnv(0x6c62_272e_07bb_0142));
    // Version 8 (custom), RFC 9562 variant.
    let hi = (hi & !0xf000) | 0x8000;
    let lo = (lo & !(0b11 << 62)) | (0b10 << 62);
    format!(
        "{:08X}-{:04X}-{:04X}-{:04X}-{:012X}",
        hi >> 32,
        (hi >> 16) & 0xffff,
        hi & 0xffff,
        lo >> 48,
        lo & 0xffff_ffff_ffff
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chrome() -> Profile {
        Profile {
            identifier: "io.github.lgibelli.mydiskisfull.test".into(),
            name: "Test & <check>".into(),
            description: "Escaping: \" ' & < >".into(),
            domain: "com.google.Chrome".into(),
            prefs: Map::from_iter([
                ("GenAILocalFoundationalModelSettings".to_string(), json!(1)),
                ("Flag".to_string(), json!(true)),
            ]),
        }
    }

    #[test]
    fn mobileconfig_is_a_valid_custom_settings_profile() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("p.mobileconfig");
        std::fs::write(&file, chrome().mobileconfig().unwrap()).unwrap();
        let lint = Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(&file)
            .output()
            .unwrap();
        assert!(
            lint.status.success(),
            "{}",
            String::from_utf8_lossy(&lint.stdout)
        );

        let top = plist::Value::from_file(&file)
            .unwrap()
            .into_dictionary()
            .unwrap();
        assert_eq!(top["PayloadType"].as_string(), Some("Configuration"));
        assert_eq!(top["PayloadScope"].as_string(), Some("User"));
        assert_eq!(
            top["PayloadDisplayName"].as_string(),
            Some("Test & <check>")
        );
        let payload = top["PayloadContent"].as_array().unwrap()[0]
            .as_dictionary()
            .unwrap();
        assert_eq!(
            payload["PayloadType"].as_string(),
            Some("com.apple.ManagedClient.preferences")
        );
        let settings = payload["PayloadContent"].as_dictionary().unwrap()["com.google.Chrome"]
            .as_dictionary()
            .unwrap()["Forced"]
            .as_array()
            .unwrap()[0]
            .as_dictionary()
            .unwrap()["mcx_preference_settings"]
            .as_dictionary()
            .unwrap();
        assert_eq!(
            settings["GenAILocalFoundationalModelSettings"].as_signed_integer(),
            Some(1)
        );
        assert_eq!(settings["Flag"].as_boolean(), Some(true));
        // Stable UUIDs, distinct per payload, well formed.
        let (a, b) = (
            top["PayloadUUID"].as_string().unwrap(),
            payload["PayloadUUID"].as_string().unwrap(),
        );
        assert_ne!(a, b);
        assert_eq!(a, uuid("io.github.lgibelli.mydiskisfull.test"));
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "8");
        assert!(matches!(&a[19..20], "8" | "9" | "A" | "B"), "{a}");
    }

    #[test]
    fn in_effect_once_every_key_is_managed() {
        let dir = tempfile::tempdir().unwrap();
        let p = chrome();
        assert!(!p.in_effect_under(dir.path(), "me"));
        let write = |path: PathBuf, gen_ai: i64| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let d = dict([
                ("GenAILocalFoundationalModelSettings", gen_ai.into()),
                ("Flag", true.into()),
                ("SomethingElse", "x".into()),
            ]);
            plist::Value::Dictionary(d).to_file_binary(path).unwrap();
        };
        // Managed for this user, with a different value: not ours.
        write(dir.path().join("me/com.google.Chrome.plist"), 0);
        assert!(!p.in_effect_under(dir.path(), "me"));
        // Our values, for the user or for the whole Mac.
        write(dir.path().join("me/com.google.Chrome.plist"), 1);
        assert!(p.in_effect_under(dir.path(), "me"));
        assert!(!p.in_effect_under(dir.path(), "someone-else"));
        write(dir.path().join("com.google.Chrome.plist"), 1);
        assert!(p.in_effect_under(dir.path(), "someone-else"));
    }
}
