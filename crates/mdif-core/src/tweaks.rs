//! Tweaks: settings that stop something from filling the disk again.
//!
//! A tweak is a list of [`Setting`] changes, worked out on the spot (the
//! Apple Intelligence opt-in keys differ between macOS versions). Applying
//! one records every value it replaces before changing anything, so it can
//! be undone exactly, and a failure halfway undoes what was done. Undoing
//! only restores settings that still hold the value the tweak set: a change
//! made since, by hand or by the app, is kept.
//!
//! Whether a tweak worked is checked later, with evidence: the settings
//! still hold (macOS updates have been known to switch things back on), the
//! restart it needed happened, and the folders it was for shrank, or grew
//! slower than before according to the watcher's history.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::forecast::DAY;
use crate::owners::Rules;
use crate::profile::Profile;
use crate::scan;
use crate::settings::{self, Setting};
use crate::store::Store;

pub struct Tweak {
    pub id: &'static str,
    pub title: &'static str,
    /// Why what it stops fills the disk.
    pub why: &'static str,
    /// What you give up.
    pub cost: &'static str,
    pub after: After,
    /// What its target folders hold, as a noun ("Chrome's on-device AI model").
    pub what: &'static str,
    /// Whether `what` is plural ("Gradle's caches").
    plural: bool,
    /// Suggested once the targets hold this much (0: whenever it applies).
    pub suggest_from: u64,
    targets: fn(&Path) -> Vec<PathBuf>,
    changes: fn(&Path) -> Result<Vec<Change>>,
    /// Why it doesn't apply to this Mac, if it doesn't.
    absent: fn(&Path) -> Option<&'static str>,
}

/// What it takes before a tweak is in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum After {
    Nothing,
    /// The app reads the setting when it starts.
    Relaunch(&'static str),
    /// macOS acts on it after a restart.
    Restart,
    /// You approve a profile in System Settings, then restart the app.
    Approve(&'static str),
}

/// A setting and the value a tweak wants (`None`: removed).
pub struct Change {
    pub setting: Setting,
    pub to: Option<Value>,
}

/// A change as made: what was there before, and what the tweak put there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub setting: Setting,
    pub from: Option<Value>,
    pub to: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Doesn't apply to this Mac, and why.
    Absent(&'static str),
    NotApplied,
    /// Some of its settings hold, not all.
    Partial,
    /// Waiting for you to approve its profile in System Settings.
    Pending,
    Applied,
}

/// Evidence that an applied tweak works.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    /// When it was first applied (and not reverted since).
    pub applied: i64,
    pub status: Status,
    /// It needs a restart that hasn't happened yet.
    pub restart_pending: bool,
    /// Each target: its size when applied, and now.
    pub sizes: Vec<(PathBuf, Option<u64>, Option<u64>)>,
    pub trend: Vec<Trend>,
}

impl Check {
    /// What the targets hold now (`None` as for [`Tweak::size`]).
    pub fn size_now(&self) -> Option<u64> {
        sum(self.sizes.iter().map(|(_, _, now)| *now))
    }
}

fn sum(sizes: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    sizes.reduce(|a, b| Some(a? + b?)).flatten()
}

/// How fast a target's owner folder grew before a tweak and since, in bytes
/// a day, when the watcher's history covers at least a day of each.
#[derive(Debug, Clone, PartialEq)]
pub struct Trend {
    pub owner: String,
    pub before: Option<f64>,
    pub after: Option<f64>,
}

/// What reverting did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Reverted {
    pub restored: Vec<Step>,
    /// Changed since the tweak set them, so left as they are.
    pub kept: Vec<(Setting, Option<Value>)>,
}

const GB: u64 = 1_000_000_000;

pub const CATALOGUE: &[Tweak] = &[
    Tweak {
        id: "apple-intelligence",
        title: "Turn Apple Intelligence off",
        why: "Apple Intelligence keeps several GB of on-device models, and macOS downloads them \
              again with updates while it is on.",
        cost: "Writing Tools, Genmoji, Image Playground and notification summaries go away.",
        after: After::Restart,
        what: "Apple Intelligence's models",
        plural: true,
        suggest_from: 0,
        // Its model folders are protected even from reading, so the effect
        // shows as free space, not as a folder that shrinks.
        targets: |_| Vec::new(),
        changes: ai_changes,
        absent: ai_absent,
    },
    Tweak {
        id: "chrome-ai-model",
        title: "Stop Chrome downloading its on-device AI model",
        why: "Chrome downloads it (about 4 GB) for its built-in AI features, and fetches it \
              again if the folder is deleted.",
        cost: "Chrome's features that run AI on this Mac stop working; nothing else changes.",
        after: After::Approve("Google Chrome"),
        what: "Chrome's on-device AI model",
        plural: false,
        suggest_from: GB,
        targets: |h| vec![h.join(CHROME_MODEL)],
        changes: |h| {
            Ok(vec![chrome_policy(
                h,
                "chrome-ai-model",
                "MyDiskIsFull: no on-device AI model in Chrome",
                "Stops Google Chrome from downloading its on-device AI model (about 4 GB).",
                "GenAILocalFoundationalModelSettings",
                json!(1),
            )])
        },
        absent: chrome_absent,
    },
    Tweak {
        id: "chrome-cache",
        title: "Cap Chrome's cache at 500 MB",
        why: "Chrome keeps a disk cache in every profile, sized by the free space it sees.",
        cost: "Sites you come back to may load a little slower.",
        after: After::Approve("Google Chrome"),
        what: "Chrome's cache",
        plural: false,
        suggest_from: 2 * GB,
        targets: |h| vec![h.join("Library/Caches/Google/Chrome")],
        changes: |h| {
            Ok(vec![chrome_policy(
                h,
                "chrome-cache",
                "MyDiskIsFull: Chrome cache at most 500 MB",
                "Caps Google Chrome's disk cache at 500 MB per profile.",
                "DiskCacheSize",
                json!(500 << 20),
            )])
        },
        absent: chrome_absent,
    },
    Tweak {
        id: "gradle-caches",
        title: "Let Gradle clear caches unused for 14 days",
        why: "Gradle keeps downloaded dependencies and Gradle distributions for 30 days after \
              their last use, and every Gradle version has a cache of its own.",
        cost: "A project not built for two weeks downloads its dependencies again.",
        after: After::Nothing,
        what: "Gradle's caches",
        plural: true,
        suggest_from: 3 * GB,
        targets: |h| {
            let g = gradle_home(h);
            vec![g.join("caches"), g.join("wrapper/dists")]
        },
        changes: |h| {
            Ok(vec![Change {
                setting: Setting::File {
                    path: gradle_home(h).join("init.d/mydiskisfull-caches.gradle"),
                },
                to: Some(Value::String(GRADLE_INIT.into())),
            }])
        },
        absent: |h| (!gradle_home(h).is_dir()).then_some("Gradle isn't used here"),
    },
    Tweak {
        id: "claude-transcripts",
        title: "Keep Claude Code transcripts for 14 days",
        why: "Claude Code keeps every session's transcript for 30 days; with agents working all \
              day they add up to gigabytes.",
        cost: "Sessions older than 14 days can't be resumed.",
        after: After::Nothing,
        what: "Claude Code's transcripts",
        plural: true,
        suggest_from: GB,
        targets: |h| vec![h.join(".claude/projects")],
        changes: |h| {
            Ok(vec![Change {
                setting: Setting::Json {
                    file: h.join(".claude/settings.json"),
                    key: "cleanupPeriodDays".into(),
                },
                to: Some(json!(14)),
            }])
        },
        absent: |h| (!h.join(".claude").is_dir()).then_some("Claude Code isn't used here"),
    },
    Tweak {
        id: "homebrew-downloads",
        title: "Let Homebrew clear downloads after 30 days",
        why: "Homebrew keeps the bottles and source archives it downloaded for 120 days.",
        cost: "Reinstalling something it already cleared downloads it again.",
        after: After::Nothing,
        what: "Homebrew's download cache",
        plural: false,
        suggest_from: GB,
        targets: |h| vec![h.join("Library/Caches/Homebrew")],
        changes: |h| {
            Ok(vec![Change {
                setting: Setting::Env {
                    file: brew_env(h),
                    key: "HOMEBREW_CLEANUP_MAX_AGE_DAYS".into(),
                },
                to: Some(json!("30")),
            }])
        },
        absent: |_| {
            (!["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
                .iter()
                .any(|b| Path::new(b).exists()))
            .then_some("Homebrew isn't installed")
        },
    },
    Tweak {
        id: "trash-auto-empty",
        title: "Empty the Trash automatically after 30 days",
        why: "Files in the Trash take space until the Trash is emptied.",
        cost: "Items deleted more than 30 days ago can't be put back.",
        after: After::Relaunch("Finder"),
        what: "the Trash",
        plural: false,
        suggest_from: GB,
        targets: |h| vec![h.join(".Trash")],
        changes: |_| {
            Ok(vec![Change {
                setting: Setting::Defaults {
                    domain: "com.apple.finder".into(),
                    key: "FXRemoveOldTrashItems".into(),
                },
                to: Some(json!(true)),
            }])
        },
        absent: |_| None,
    },
];

pub fn find(id: &str) -> Option<&'static Tweak> {
    CATALOGUE.iter().find(|t| t.id == id)
}

impl Tweak {
    pub fn targets(&self, home: &Path) -> Vec<PathBuf> {
        (self.targets)(home)
    }

    /// What its targets hold now, every file once (`None` when there's
    /// nothing to measure or one can't be read).
    pub fn size(&self, home: &Path) -> Option<u64> {
        sum(self.targets(home).iter().map(|p| scan::size_of(p)))
    }

    /// "Gradle's caches take 8.8 GB", capitalized.
    pub fn takes(&self, size: u64) -> String {
        let mut c = self.what.chars();
        let what: String = c
            .next()
            .map(|f| f.to_uppercase().chain(c).collect())
            .unwrap_or_default();
        let verb = if self.plural { "take" } else { "takes" };
        format!("{what} {verb} {}", crate::units::bytes(size))
    }

    pub fn status(&self, home: &Path) -> Result<Status> {
        if let Some(why) = (self.absent)(home) {
            return Ok(Status::Absent(why));
        }
        let changes = (self.changes)(home)?;
        let mut held = 0;
        let mut pending = false;
        for c in &changes {
            if c.setting.read()? == c.to {
                held += 1;
            } else if let Setting::Profile { profile, dir } = &c.setting {
                pending |= profile.downloaded(dir);
            }
        }
        Ok(if held == changes.len() {
            Status::Applied
        } else if pending {
            Status::Pending
        } else if held == 0 {
            Status::NotApplied
        } else {
            Status::Partial
        })
    }

    /// The steps applying it would take now; none when it is applied.
    pub fn plan(&self, home: &Path) -> Result<Vec<Step>> {
        let mut steps = Vec::new();
        for c in (self.changes)(home)? {
            let from = c.setting.read()?;
            if from != c.to {
                steps.push(Step {
                    setting: c.setting,
                    from,
                    to: c.to,
                });
            }
        }
        Ok(steps)
    }

    /// Apply it, recording what it replaces first. Returns the steps taken.
    pub fn apply(&self, home: &Path, store: &Store, now: i64) -> Result<Vec<Step>> {
        if let Some(why) = (self.absent)(home) {
            anyhow::bail!("{} doesn't apply: {why}", self.id);
        }
        let steps = self.plan(home)?;
        if steps.is_empty() {
            return Ok(steps);
        }
        mdif_macos::fs::forbid_dataless_materialization()?;
        let sizes: Vec<(PathBuf, Option<u64>)> = self
            .targets(home)
            .into_iter()
            .map(|p| {
                let s = scan::size_of(&p);
                (p, s)
            })
            .collect();
        store.add_tweak(
            self.id,
            now,
            &serde_json::to_string(&steps)?,
            &serde_json::to_string(&sizes)?,
        )?;
        for (i, s) in steps.iter().enumerate() {
            if let Err(e) = s.setting.write(s.to.as_ref()) {
                // Undo this step too: it may have half happened (a profile
                // saved but not opened).
                for done in steps[..=i].iter().rev() {
                    let _ = done.setting.write(done.from.as_ref());
                }
                let e = e.context(format!("{}: {}", self.id, s.setting));
                // Only this attempt's record: earlier ones still hold.
                return Err(match store.forget_tweak(self.id, now) {
                    Ok(()) => e,
                    Err(f) => e.context(format!("and its record couldn't be removed: {f:#}")),
                });
            }
        }
        Ok(steps)
    }

    /// Put back what it replaced, for every time it was applied, most
    /// recent first. A setting changed since keeps its new value.
    pub fn revert(&self, store: &Store, now: i64) -> Result<Reverted> {
        let records = store.open_tweaks(self.id)?;
        anyhow::ensure!(
            !records.is_empty(),
            "MyDiskIsFull has no record of applying {}, so it doesn't know what was there \
             before; change it back by hand",
            self.id
        );
        let mut out = Reverted::default();
        for r in &records {
            let steps: Vec<Step> = serde_json::from_str(&r.steps)
                .with_context(|| format!("reading the record of {}", self.id))?;
            for s in steps.into_iter().rev() {
                let now_value = s.setting.read()?;
                // A profile reads as absent until it's approved, but its
                // download is ours to remove all the same.
                let downloaded = matches!(&s.setting, Setting::Profile { profile, dir }
                    if s.from.is_none() && profile.downloaded(dir));
                if now_value == s.to || downloaded {
                    s.setting.write(s.from.as_ref())?;
                    out.restored.push(s);
                } else if now_value != s.from {
                    out.kept.push((s.setting, now_value));
                }
            }
        }
        store.revert_tweak(self.id, now)?;
        Ok(out)
    }

    /// The evidence for a tweak applied with MyDiskIsFull (`None` if it
    /// wasn't, or has been reverted).
    pub fn check(&self, home: &Path, store: &Store, now: i64) -> Result<Option<Check>> {
        let records = store.open_tweaks(self.id)?;
        let (Some(first), Some(latest)) = (records.last(), records.first()) else {
            return Ok(None);
        };
        mdif_macos::fs::forbid_dataless_materialization()?;
        let then: Vec<(PathBuf, Option<u64>)> =
            serde_json::from_str(&first.sizes).unwrap_or_default();
        let sizes = then
            .into_iter()
            .map(|(p, s)| {
                let now = scan::size_of(&p);
                (p, s, now)
            })
            .collect::<Vec<_>>();
        let restart_pending = self.after == After::Restart
            && mdif_macos::sys::boot_time().is_ok_and(|b| b < latest.applied);
        Ok(Some(Check {
            applied: first.applied,
            status: self.status(home)?,
            restart_pending,
            trend: trend(store, home, &sizes, first.applied, now)?,
            sizes,
        }))
    }
}

/// Growth a day of each target's owner folder in the week before `applied`
/// and since, from the watcher's history.
fn trend(
    store: &Store,
    home: &Path,
    sizes: &[(PathBuf, Option<u64>, Option<u64>)],
    applied: i64,
    now: i64,
) -> Result<Vec<Trend>> {
    let rules = Rules::for_home(home);
    let mut owners: Vec<String> = sizes
        .iter()
        .map(|(p, _, _)| rules.owner_of(p).display().to_string())
        .collect();
    owners.sort();
    owners.dedup();
    let series = store.growth_series(applied - 7 * DAY)?;
    let sampled = store.sample_times(applied - 7 * DAY)?;
    // The part of (a, b] the watcher was running for, if at least a day.
    let covered = |a: i64, b: i64| {
        let (lo, hi) = sampled
            .iter()
            .filter(|&&t| t > a && t <= b)
            .fold((i64::MAX, i64::MIN), |(lo, hi), &t| (lo.min(t), hi.max(t)));
        (hi > lo && hi - lo >= DAY).then_some((lo, hi))
    };
    let rate = |rows: &[(i64, i64)], a: i64, b: i64| {
        let (lo, hi) = covered(a, b)?;
        let sum: i64 = rows
            .iter()
            .filter(|(t, _)| *t > lo && *t <= hi)
            .map(|(_, g)| g)
            .sum();
        Some(sum as f64 * DAY as f64 / (hi - lo) as f64)
    };
    Ok(owners
        .into_iter()
        .map(|o| {
            let rows = series.get(&o).map(Vec::as_slice).unwrap_or_default();
            Trend {
                before: rate(rows, applied - 7 * DAY, applied),
                after: rate(rows, applied, now),
                owner: o,
            }
        })
        .collect())
}

pub const CHROME_MODEL: &str = "Library/Application Support/Google/Chrome/OptGuideOnDeviceModel";

const AI_DOMAIN: &str = "com.apple.CloudSubscriptionFeatures.optIn";

/// The opt-in flags: a numeric key per feature set (the number differs
/// between macOS versions), and `auto_opt_in`, which would switch them back
/// on by itself.
fn ai_keys(domain: &Map<String, Value>) -> Vec<String> {
    let mut keys: Vec<String> = domain
        .iter()
        .filter(|(k, v)| v.is_boolean() && !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit()))
        .map(|(k, _)| k.clone())
        .collect();
    keys.push("auto_opt_in".into());
    keys
}

fn ai_changes(_: &Path) -> Result<Vec<Change>> {
    Ok(ai_keys(&settings::defaults(AI_DOMAIN)?)
        .into_iter()
        .map(|key| Change {
            setting: Setting::Defaults {
                domain: AI_DOMAIN.into(),
                key,
            },
            to: Some(Value::Bool(false)),
        })
        .collect())
}

fn ai_absent(_: &Path) -> Option<&'static str> {
    match settings::defaults(AI_DOMAIN) {
        Ok(d) if ai_keys(&d).iter().any(|k| d.contains_key(k)) => None,
        _ => Some("Apple Intelligence has never been on here"),
    }
}

fn chrome_absent(home: &Path) -> Option<&'static str> {
    let app = "Applications/Google Chrome.app";
    (!(Path::new("/").join(app).exists() || home.join(app).exists()))
        .then_some("Google Chrome isn't installed")
}

/// Where downloaded profiles are kept until they're removed.
pub fn profiles_dir(home: &Path) -> PathBuf {
    home.join("Library/Application Support/MyDiskIsFull/Profiles")
}

fn chrome_policy(
    home: &Path,
    id: &str,
    name: &str,
    description: &str,
    key: &str,
    value: Value,
) -> Change {
    Change {
        setting: Setting::Profile {
            profile: Profile {
                identifier: format!("io.github.lgibelli.mydiskisfull.{id}"),
                name: name.into(),
                description: description.into(),
                domain: "com.google.Chrome".into(),
                prefs: Map::from_iter([(key.to_string(), value)]),
            },
            dir: profiles_dir(home),
        },
        to: Some(Value::Bool(true)),
    }
}

fn gradle_home(home: &Path) -> PathBuf {
    std::env::var_os("GRADLE_USER_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(|| home.join(".gradle"), PathBuf::from)
}

/// Homebrew's user `brew.env`, found the way `brew` itself finds it.
fn brew_env(home: &Path) -> PathBuf {
    let var = |k| std::env::var_os(k).filter(|v| !v.is_empty());
    match var("XDG_CONFIG_HOME").or_else(|| var("HOMEBREW_XDG_CONFIG_HOME")) {
        Some(x) => PathBuf::from(x).join("homebrew/brew.env"),
        None => home.join(".homebrew/brew.env"),
    }
}

/// Cache retention for every build, from Gradle 8.0 on (older versions
/// skip it rather than fail). Checked with Gradle 8.11 and 8.14: downloads
/// and distributions go from 30 days to 14, the rest stays at 7.
const GRADLE_INIT: &str = "\
// Written by MyDiskIsFull; `mdif tweaks revert gradle-caches` removes it.
// Gradle deletes downloads and Gradle distributions unused for this many
// days (default 30), at most once a day.
if (GradleVersion.current() >= GradleVersion.version('8.0')) {
    beforeSettings { settings ->
        settings.caches {
            releasedWrappers.removeUnusedEntriesAfterDays = 14
            downloadedResources.removeUnusedEntriesAfterDays = 14
        }
    }
}
";

#[cfg(test)]
mod tests {
    use super::*;

    /// A tweak over files in a scratch home, so tests touch nothing real.
    fn files_tweak() -> Tweak {
        Tweak {
            id: "test-files",
            title: "",
            why: "",
            cost: "",
            after: After::Nothing,
            what: "the test folder",
            plural: false,
            suggest_from: 0,
            targets: |h| vec![h.join("data")],
            changes: |h| {
                Ok(vec![
                    Change {
                        setting: Setting::Json {
                            file: h.join("settings.json"),
                            key: "days".into(),
                        },
                        to: Some(json!(14)),
                    },
                    Change {
                        setting: Setting::File {
                            path: h.join("init.d/x"),
                        },
                        to: Some(json!("on\n")),
                    },
                ])
            },
            absent: |_| None,
        }
    }

    #[test]
    fn apply_then_revert_puts_everything_back() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir(h.join("data")).unwrap();
        std::fs::write(h.join("data/f"), vec![1u8; 1 << 20]).unwrap();
        std::fs::write(
            h.join("settings.json"),
            "{\n  \"days\": 30,\n  \"other\": 1\n}\n",
        )
        .unwrap();
        let store = Store::open_in_memory().unwrap();
        let t = files_tweak();
        assert_eq!(t.status(h).unwrap(), Status::NotApplied);

        let steps = t.apply(h, &store, 1000).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].from, Some(json!(30)));
        assert_eq!(steps[1].from, None);
        assert_eq!(t.status(h).unwrap(), Status::Applied);
        assert!(
            t.apply(h, &store, 1100).unwrap().is_empty(),
            "already applied"
        );

        let check = t.check(h, &store, 2000).unwrap().unwrap();
        assert_eq!(check.applied, 1000);
        assert_eq!(check.status, Status::Applied);
        let (_, then, now) = &check.sizes[0];
        assert!(then.unwrap() >= 1 << 20 && then == now, "{check:?}");

        let r = t.revert(&store, 3000).unwrap();
        assert_eq!(r.restored.len(), 2);
        assert!(r.kept.is_empty());
        assert_eq!(
            std::fs::read_to_string(h.join("settings.json")).unwrap(),
            "{\n  \"days\": 30,\n  \"other\": 1\n}\n"
        );
        assert!(!h.join("init.d/x").exists());
        assert_eq!(t.status(h).unwrap(), Status::NotApplied);
        assert!(t.check(h, &store, 3000).unwrap().is_none());
        assert!(t.revert(&store, 3100).is_err(), "nothing left to revert");
    }

    #[test]
    fn revert_keeps_what_was_changed_since() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let store = Store::open_in_memory().unwrap();
        let t = files_tweak();
        t.apply(h, &store, 1000).unwrap();
        // The user picks another value by hand.
        let days = Setting::Json {
            file: h.join("settings.json"),
            key: "days".into(),
        };
        days.write(Some(&json!(7))).unwrap();
        assert_eq!(t.status(h).unwrap(), Status::Partial);
        let r = t.revert(&store, 2000).unwrap();
        assert_eq!(r.kept, [(days.clone(), Some(json!(7)))]);
        assert_eq!(r.restored.len(), 1);
        assert_eq!(days.read().unwrap(), Some(json!(7)));
    }

    #[test]
    fn reapplying_after_a_reset_reverts_to_the_original() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::write(h.join("settings.json"), "{\"days\": 30}").unwrap();
        let store = Store::open_in_memory().unwrap();
        let t = files_tweak();
        t.apply(h, &store, 1000).unwrap();
        // Something switches one setting back, and the tweak is applied again.
        let days = Setting::Json {
            file: h.join("settings.json"),
            key: "days".into(),
        };
        days.write(Some(&json!(30))).unwrap();
        assert_eq!(t.apply(h, &store, 2000).unwrap().len(), 1);
        t.revert(&store, 3000).unwrap();
        assert_eq!(days.read().unwrap(), Some(json!(30)));
        assert!(!h.join("init.d/x").exists());
        // The first record holds the original sizes.
        assert!(store.open_tweaks("test-files").unwrap().is_empty());
    }

    #[test]
    fn a_failed_step_undoes_the_ones_before() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        // The second step can't be written: its folder is a file.
        std::fs::write(h.join("init.d"), "not a folder").unwrap();
        let store = Store::open_in_memory().unwrap();
        let t = files_tweak();
        assert!(t.apply(h, &store, 1000).is_err());
        let days = Setting::Json {
            file: h.join("settings.json"),
            key: "days".into(),
        };
        assert_eq!(days.read().unwrap(), None, "first step undone");
        assert!(store.open_tweaks("test-files").unwrap().is_empty());
    }

    #[test]
    fn a_failed_reapply_keeps_the_earlier_record() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let store = Store::open_in_memory().unwrap();
        let t = files_tweak();
        use std::os::unix::fs::PermissionsExt;
        t.apply(h, &store, 1000).unwrap();
        // The JSON step is reset, and its folder can't be written any more.
        let days = Setting::Json {
            file: h.join("settings.json"),
            key: "days".into(),
        };
        days.write(Some(&json!(30))).unwrap();
        let mode = |m| std::fs::set_permissions(h, std::fs::Permissions::from_mode(m)).unwrap();
        mode(0o555);
        let failed = t.apply(h, &store, 2000);
        mode(0o755);
        assert!(failed.is_err());
        let open = store.open_tweaks("test-files").unwrap();
        assert_eq!(open.iter().map(|r| r.applied).collect::<Vec<_>>(), [1000]);
        // Reverting still knows what the first apply replaced.
        let r = t.revert(&store, 3000).unwrap();
        assert_eq!(r.restored.len(), 1, "{r:?}");
        assert!(!h.join("init.d/x").exists());
    }

    #[test]
    fn reverting_a_profile_never_approved_removes_its_download() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("Profiles");
        let profile = Profile {
            identifier: "io.github.lgibelli.mydiskisfull.test-never-approved".into(),
            name: "test".into(),
            description: "test".into(),
            domain: "io.github.lgibelli.mydiskisfull.test".into(),
            prefs: Map::from_iter([("k".to_string(), json!(1))]),
        };
        // What apply leaves when the user hasn't approved it: the download.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(profile.file_in(&dir), profile.mobileconfig().unwrap()).unwrap();
        let step = Step {
            setting: Setting::Profile {
                profile: profile.clone(),
                dir: dir.clone(),
            },
            from: None,
            to: Some(json!(true)),
        };
        let store = Store::open_in_memory().unwrap();
        let t = Tweak {
            id: "test-profile",
            ..files_tweak()
        };
        store
            .add_tweak(t.id, 1000, &serde_json::to_string(&[step]).unwrap(), "[]")
            .unwrap();
        let r = t.revert(&store, 2000).unwrap();
        assert_eq!(r.restored.len(), 1);
        assert!(!profile.file_in(&dir).exists());
    }

    #[test]
    fn trends_are_rates_over_the_time_watched() {
        let store = Store::open_in_memory().unwrap();
        let home = Path::new("/Users/me");
        let owner = "/Users/me/.gradle";
        let applied = 30 * DAY;
        // Watched for the 2 days before (20 GB grown) and the 2 days since (1 GB).
        for i in 0..=192 {
            let ts = applied - 2 * DAY + i * 1800;
            let p = crate::store::SpacePoint {
                ts,
                total: 1,
                avail: 1,
            };
            store.add_space("/", p).unwrap();
            let grown = if ts <= applied {
                20_000_000_000 / 96
            } else {
                1_000_000_000 / 96
            };
            if i > 0 {
                store.add_growth(ts, &[(owner.to_string(), grown)]).unwrap();
            }
        }
        let sizes = [(PathBuf::from("/Users/me/.gradle/caches"), None, None)];
        let tr = trend(&store, home, &sizes, applied, applied + 2 * DAY).unwrap();
        assert_eq!(tr[0].owner, owner);
        let (before, after) = (tr[0].before.unwrap(), tr[0].after.unwrap());
        assert!((before / 1e9 - 10.0).abs() < 0.6, "{before}");
        assert!((after / 1e9 - 0.5).abs() < 0.1, "{after}");
    }

    #[test]
    fn apple_intelligence_keys_are_the_numeric_flags_and_auto_opt_in() {
        let d: Map<String, Value> = serde_json::from_value(json!({
            "109547227": true,
            "545129924": false,
            "auto_opt_in": true,
            "opted_in_buddy": true,
            "12": "not a flag",
        }))
        .unwrap();
        let mut keys = ai_keys(&d);
        keys.sort();
        assert_eq!(keys, ["109547227", "545129924", "auto_opt_in"]);
    }

    #[test]
    fn catalogue_ids_are_unique_and_kebab_case() {
        let mut ids: Vec<&str> = CATALOGUE.iter().map(|t| t.id).collect();
        assert!(
            ids.iter()
                .all(|id| id.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
        );
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), CATALOGUE.len());
        for t in CATALOGUE {
            assert!(t.why.ends_with('.') && t.cost.ends_with('.'), "{}", t.id);
        }
    }

    #[test]
    fn every_tweak_reads_its_state_without_changing_anything() {
        // The real settings of whoever runs the tests: read, never written.
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
        let profiles = profiles_dir(&home).exists();
        for t in CATALOGUE {
            let status = t.status(&home).unwrap();
            if matches!(status, Status::Absent(_)) {
                continue;
            }
            let before = t.plan(&home).unwrap();
            assert_eq!(t.status(&home).unwrap(), status, "{}", t.id);
            assert_eq!(t.plan(&home).unwrap(), before, "{}", t.id);
            assert_eq!(before.is_empty(), status == Status::Applied, "{}", t.id);
        }
        assert_eq!(profiles_dir(&home).exists(), profiles, "no profile written");
    }
}
