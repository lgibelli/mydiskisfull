//! Settings a tweak may change, and how to put each one back.
//!
//! A [`Setting`] is one value somewhere on the Mac: a user default, a key of
//! a JSON settings file, a line of an env file, a whole file, or one of our
//! configuration profiles. Each reads as `None` when absent, and writing
//! `None` removes it, so the value read before a change is all it takes to
//! undo the change exactly.
//!
//! Files are replaced atomically (written beside, then renamed) with their
//! permissions kept, and a symlinked file (a dotfiles repo) is written
//! through the link, never replaced by a copy.

use std::fmt;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::profile::Profile;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Setting {
    /// A user default, read and written with `defaults` so cfprefsd knows:
    /// editing the plist behind its back doesn't reliably stick.
    Defaults { domain: String, key: String },
    /// A top-level key of a JSON file. The other keys and their order stay.
    Json { file: PathBuf, key: String },
    /// `KEY=value` in an env file such as Homebrew's `brew.env`, which takes
    /// the value verbatim: no `export`, no quotes.
    Env { file: PathBuf, key: String },
    /// A whole file, as a string.
    File { path: PathBuf },
    /// One of our configuration profiles, saved in `dir` when downloaded.
    /// Reads as `true` once installed and in effect.
    Profile { profile: Profile, dir: PathBuf },
}

impl Setting {
    pub fn read(&self) -> Result<Option<Value>> {
        match self {
            Setting::Defaults { domain, key } => Ok(defaults(domain)?.remove(key)),
            Setting::Json { file, key } => Ok(json_object(file)?.and_then(|mut o| o.remove(key))),
            Setting::Env { file, key } => Ok(read_text(file)?.and_then(|t| env_value(&t, key))),
            Setting::File { path } => Ok(read_text(path)?.map(Value::String)),
            Setting::Profile { profile, .. } => {
                Ok(profile.in_effect().then_some(Value::Bool(true)))
            }
        }
    }

    /// Set it to `to`, or remove it when `to` is `None`.
    pub fn write(&self, to: Option<&Value>) -> Result<()> {
        match self {
            Setting::Defaults { domain, key } => write_default(domain, key, to),
            Setting::Json { file, key } => {
                let mut obj = json_object(file)?.unwrap_or_default();
                match to {
                    Some(v) => {
                        obj.insert(key.clone(), v.clone());
                    }
                    None => {
                        obj.shift_remove(key);
                    }
                }
                let mut text = serde_json::to_string_pretty(&obj)?;
                text.push('\n');
                replace_file(file, &text)
            }
            Setting::Env { file, key } => {
                let value = match to {
                    None => None,
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(Value::Number(n)) => Some(n.to_string()),
                    Some(v) => bail!("{key}: an env file can't hold {v}"),
                };
                if let Some(v) = &value {
                    ensure!(!v.contains('\n'), "{key}: one line only");
                }
                let text = read_text(file)?.unwrap_or_default();
                let new = set_env(&text, key, value.as_deref());
                if new.trim().is_empty() {
                    remove_file(file)
                } else {
                    replace_file(file, &new)
                }
            }
            Setting::File { path } => match to {
                None => remove_file(path),
                Some(Value::String(s)) => replace_file(path, s),
                Some(v) => bail!("{}: a file holds text, not {v}", path.display()),
            },
            Setting::Profile { profile, dir } => match to {
                Some(Value::Bool(true)) => profile.download(dir).map(|_| ()),
                None => profile.remove(dir),
                Some(v) => bail!(
                    "{}: a profile is installed or not, not {v}",
                    profile.identifier
                ),
            },
        }
    }
}

impl fmt::Display for Setting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Setting::Defaults { domain, key } => write!(f, "defaults {domain} {key}"),
            Setting::Json { file, key } | Setting::Env { file, key } => {
                write!(f, "{}: {key}", file.display())
            }
            Setting::File { path } => write!(f, "{}", path.display()),
            Setting::Profile { profile, .. } => write!(f, "profile \"{}\"", profile.name),
        }
    }
}

/// A user-defaults domain's values (`defaults export`), the kinds a setting
/// can hold: booleans, numbers and strings. A domain that doesn't exist is
/// empty.
pub fn defaults(domain: &str) -> Result<Map<String, Value>> {
    let out = Command::new("/usr/bin/defaults")
        .args(["export", domain, "-"])
        .output()
        .context("running defaults")?;
    ensure!(
        out.status.success(),
        "defaults export {domain}: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let dict = plist::Value::from_reader_xml(&out.stdout[..])
        .with_context(|| format!("parsing defaults {domain}"))?
        .into_dictionary()
        .with_context(|| format!("defaults {domain} is not a dictionary"))?;
    Ok(dict
        .into_iter()
        .filter_map(|(k, v)| scalar(&v).map(|v| (k, v)))
        .collect())
}

fn scalar(v: &plist::Value) -> Option<Value> {
    match v {
        plist::Value::Boolean(b) => Some(Value::Bool(*b)),
        plist::Value::Integer(i) => i.as_signed().map(Value::from),
        plist::Value::Real(r) => serde_json::Number::from_f64(*r).map(Value::Number),
        plist::Value::String(s) => Some(Value::String(s.clone())),
        _ => None,
    }
}

fn write_default(domain: &str, key: &str, to: Option<&Value>) -> Result<()> {
    let mut cmd = Command::new("/usr/bin/defaults");
    match to {
        None => {
            if !defaults(domain)?.contains_key(key) {
                return Ok(());
            }
            cmd.args(["delete", domain, key]);
        }
        Some(v) => {
            let (kind, text) = match v {
                Value::Bool(b) => ("-bool", b.to_string()),
                Value::Number(n) if n.is_f64() => ("-float", n.to_string()),
                Value::Number(n) => ("-int", n.to_string()),
                Value::String(s) => ("-string", s.clone()),
                v => bail!("defaults {domain} {key}: can't write {v}"),
            };
            cmd.args(["write", domain, key, kind, &text]);
        }
    }
    let out = cmd.output().context("running defaults")?;
    ensure!(
        out.status.success(),
        "defaults {domain} {key}: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(())
}

/// The file's JSON object, or `None` when the file doesn't exist. Anything
/// else (invalid JSON, comments, an array) is an error: never overwrite a
/// file we don't understand.
fn json_object(file: &Path) -> Result<Option<Map<String, Value>>> {
    let Some(text) = read_text(file)? else {
        return Ok(None);
    };
    if text.trim().is_empty() {
        return Ok(Some(Map::new()));
    }
    match serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON; leaving it alone", file.display()))?
    {
        Value::Object(o) => Ok(Some(o)),
        _ => bail!("{} is not a JSON object; leaving it alone", file.display()),
    }
}

/// The last assignment to `key` wins, as when the file is read line by line.
fn env_value(text: &str, key: &str) -> Option<Value> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix(key)?.strip_prefix('='))
        .next_back()
        .map(|v| Value::String(v.to_string()))
}

/// `text` with `key` set to `value` in place of its first assignment (later
/// ones removed), appended if it had none, or removed when `None`.
fn set_env(text: &str, key: &str, value: Option<&str>) -> String {
    let assigns = |l: &str| {
        l.trim()
            .strip_prefix(key)
            .is_some_and(|r| r.starts_with('='))
    };
    let mut out = Vec::new();
    let mut done = false;
    for line in text.lines() {
        if assigns(line) {
            if let (Some(v), false) = (value, done) {
                out.push(format!("{key}={v}"));
            }
            done = true;
        } else {
            out.push(line.to_string());
        }
    }
    if let (Some(v), false) = (value, done) {
        out.push(format!("{key}={v}"));
    }
    let mut s = out.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

fn read_text(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write `text` to `path` atomically, through a symlink if it is one,
/// keeping the old file's permissions.
fn replace_file(path: &Path, text: &str) -> Result<()> {
    let path = match std::fs::canonicalize(path) {
        Ok(real) => real,
        Err(e) if e.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
        Err(e) => return Err(e).with_context(|| format!("resolving {}", path.display())),
    };
    let dir = path
        .parent()
        .with_context(|| format!("{} has no folder", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(".{name}.mdif-{}", std::process::id()));
    // Created with the old file's mode, so a private file is never readable
    // by others, even for a moment.
    let mode = std::fs::metadata(&path).map_or(0o644, |m| m.permissions().mode() & 0o7777);
    let written = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        f.write_all(text.as_bytes())?;
        // The umask may have narrowed it.
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        std::fs::rename(&tmp, &path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.with_context(|| format!("replacing {}", path.display()))
}

fn remove_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("removing {}", path.display()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Write `to`, check it reads back, then put `before` back and check
    /// the setting is exactly as it was.
    fn round_trip(s: &Setting, to: Value) {
        let before = s.read().unwrap();
        s.write(Some(&to)).unwrap();
        assert_eq!(s.read().unwrap(), Some(to), "{s}");
        s.write(before.as_ref()).unwrap();
        assert_eq!(s.read().unwrap(), before, "{s} restored");
    }

    #[test]
    fn json_keys_change_alone_and_keep_their_order() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        let original =
            "{\n  \"zeta\": 1,\n  \"alpha\": {\n    \"x\": [\n      true\n    ]\n  }\n}\n";
        std::fs::write(&file, original).unwrap();
        let s = Setting::Json {
            file: file.clone(),
            key: "cleanupPeriodDays".into(),
        };
        assert_eq!(s.read().unwrap(), None);
        s.write(Some(&json!(14))).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            text.find("zeta").unwrap() < text.find("alpha").unwrap()
                && text.find("alpha").unwrap() < text.find("cleanupPeriodDays").unwrap(),
            "{text}"
        );
        s.write(None).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
        round_trip(&s, json!(7));
    }

    #[test]
    fn json_that_isnt_an_object_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        for bad in ["[1, 2]", "{ // a comment\n}", "{\"a\": "] {
            std::fs::write(&file, bad).unwrap();
            let s = Setting::Json {
                file: file.clone(),
                key: "k".into(),
            };
            assert!(s.write(Some(&json!(1))).is_err(), "{bad}");
            assert_eq!(std::fs::read_to_string(&file).unwrap(), bad);
        }
    }

    #[test]
    fn json_is_written_through_a_symlink_with_its_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles-settings.json");
        std::fs::write(&real, "{}").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let s = Setting::Json {
            file: link.clone(),
            key: "k".into(),
        };
        s.write(Some(&json!("v"))).unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(
            std::fs::read_to_string(&real)
                .unwrap()
                .contains("\"k\": \"v\"")
        );
        let mode = std::fs::metadata(&real).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        // Nothing left beside it.
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    #[test]
    fn env_lines_are_replaced_in_place_and_removed_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".homebrew/brew.env");
        let key = "HOMEBREW_CLEANUP_MAX_AGE_DAYS";
        let s = Setting::Env {
            file: file.clone(),
            key: key.into(),
        };
        // Created with its folder, and removed again when it ends up empty.
        round_trip(&s, json!("30"));
        assert!(!file.exists());
        // Next to other settings: replaced where it was, the rest untouched.
        let other = "HOMEBREW_NO_ANALYTICS=1\nHOMEBREW_CLEANUP_MAX_AGE_DAYS=60\n# note\n";
        std::fs::write(&file, other).unwrap();
        assert_eq!(s.read().unwrap(), Some(json!("60")));
        s.write(Some(&json!("30"))).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "HOMEBREW_NO_ANALYTICS=1\nHOMEBREW_CLEANUP_MAX_AGE_DAYS=30\n# note\n"
        );
        s.write(Some(&json!("60"))).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), other);
        // A key that only starts the same way is another key.
        let near = Setting::Env {
            file: file.clone(),
            key: "HOMEBREW_CLEANUP".into(),
        };
        assert_eq!(near.read().unwrap(), None);
    }

    #[test]
    fn files_are_created_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("init.d/x.gradle");
        let s = Setting::File { path: path.clone() };
        round_trip(&s, json!("// x\n"));
        assert!(!path.exists());
    }

    /// A scratch defaults domain, removed with its plist even if the test fails.
    struct Scratch(String);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = Command::new("/usr/bin/defaults")
                .args(["delete", &self.0])
                .output();
            if let Some(home) = std::env::var_os("HOME") {
                let plist = Path::new(&home).join(format!("Library/Preferences/{}.plist", self.0));
                let _ = std::fs::remove_file(plist);
            }
        }
    }

    #[test]
    fn user_defaults_round_trip_through_cfprefsd() {
        let scratch = Scratch(format!(
            "io.github.lgibelli.mydiskisfull.test-{}",
            std::process::id()
        ));
        let domain = scratch.0.clone();
        let s = Setting::Defaults {
            domain: domain.clone(),
            key: "12345".into(),
        };
        assert_eq!(s.read().unwrap(), None);
        round_trip(&s, json!(false));
        round_trip(&s, json!(30));
        round_trip(&s, json!("text"));
        s.write(Some(&json!(true))).unwrap();
        assert_eq!(defaults(&domain).unwrap()["12345"], json!(true));
    }
}
