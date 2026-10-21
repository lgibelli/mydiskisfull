//! `mdif tweaks` (settings that stop something coming back, applied and
//! undone) and `mdif suggest` (what to do about what fills the disk).

use std::path::Path;

use anyhow::{Context, Result, bail};
use mdif_core::profile::{self, Profile};
use mdif_core::settings::Setting;
use mdif_core::store::Store;
use mdif_core::suggest::{self, Fix};
use mdif_core::text::printable;
use mdif_core::tweaks::{self, After, CATALOGUE, Check, Status, Step, Tweak};
use mdif_core::units::bytes;
use mdif_core::watch::unix_now;
use serde_json::{Value, json};

use crate::watch_cmd::{home, local, tilde};

fn store(db: &Path) -> Result<Option<Store>> {
    Ok(if db.exists() {
        Some(Store::open(db)?)
    } else {
        None
    })
}

fn find(id: &str) -> Result<&'static Tweak> {
    tweaks::find(id).with_context(|| {
        let ids: Vec<&str> = CATALOGUE.iter().map(|t| t.id).collect();
        format!("no tweak \"{id}\"; there are: {}", ids.join(", "))
    })
}

pub fn list(db: &Path, json_out: bool) -> Result<()> {
    mdif_macos::fs::forbid_dataless_materialization()?;
    let home = home();
    let store = store(db)?;
    let now = unix_now();
    let mut rows = Vec::new();
    for t in CATALOGUE {
        // One setting that can't be read (a settings file with comments,
        // say) is shown as such; it doesn't hide the other tweaks.
        let (status, check) = match t.status(&home).and_then(|st| {
            let check = match &store {
                Some(s) => t.check(&home, s, now)?,
                None => None,
            };
            Ok((st, check))
        }) {
            Ok(x) => x,
            Err(e) => {
                if json_out {
                    continue;
                }
                println!();
                println!("{:<20}{}", t.id, t.title);
                println!(
                    "{:<20}Can't read its settings: {}",
                    "",
                    printable(&format!("{e:#}"))
                );
                continue;
            }
        };
        let size = check
            .as_ref()
            .and_then(|c| c.size_now())
            .or_else(|| t.size(&home));
        rows.push((t, status, check, size));
    }
    if json_out {
        let out: Vec<_> = rows
            .iter()
            .map(|(t, status, check, size)| {
                json!({
                    "id": t.id,
                    "title": t.title,
                    "status": status_word(*status),
                    "why": t.why,
                    "cost": t.cost,
                    "size": size,
                    "applied": check.as_ref().map(|c| c.applied),
                    "restart_pending": check.as_ref().map(|c| c.restart_pending),
                    "sizes": check.as_ref().map(|c| c.sizes.iter().map(|(p, a, b)| json!({
                        "path": p, "then": a, "now": b,
                    })).collect::<Vec<_>>()),
                    "trend": check.as_ref().map(|c| c.trend.iter().map(|t| json!({
                        "owner": t.owner, "per_day_before": t.before, "per_day_after": t.after,
                    })).collect::<Vec<_>>()),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!("Each tweak stops something from filling the disk again, and can be undone.");
    for (t, status, check, size) in &rows {
        println!();
        println!("{:<20}{}", t.id, t.title);
        println!(
            "{:<20}{}",
            "",
            state_line(t, *status, check.as_ref(), *size)
        );
        if let Some(c) = check {
            for line in evidence(t, c) {
                println!("{:<20}{line}", "");
            }
        }
    }
    println!();
    println!("`mdif tweaks apply <id> --dry-run` shows what one changes, and what you give up.");
    Ok(())
}

pub(crate) fn status_word(s: Status) -> &'static str {
    match s {
        Status::Absent(_) => "absent",
        Status::NotApplied => "not applied",
        Status::Partial => "partly applied",
        Status::Pending => "waiting for approval",
        Status::Applied => "applied",
    }
}

fn state_line(t: &Tweak, status: Status, check: Option<&Check>, size: Option<u64>) -> String {
    let what = || {
        size.filter(|_| !t.targets(&home()).is_empty())
            .map(|s| format!(" {}.", t.takes(s)))
            .unwrap_or_default()
    };
    match (status, check) {
        (Status::Absent(why), _) => format!("Doesn't apply: {why}."),
        (Status::NotApplied, None) => format!("Not applied.{}", what()),
        (Status::NotApplied | Status::Partial, Some(c)) => format!(
            "Applied {} but undone since, by an update or the app.{} Apply it again to restore it.",
            local(c.applied).format("%b %-d"),
            what()
        ),
        (Status::Partial, None) => format!("Partly applied.{}", what()),
        (Status::Pending, _) => format!(
            "Waiting for you to install its profile in System Settings → {}. Nothing changes \
             until you do.",
            profile::settings_place()
        ),
        (Status::Applied, None) => "Applied (not by MyDiskIsFull, so it can't undo it).".into(),
        (Status::Applied, Some(c)) if c.restart_pending => format!(
            "Applied {}; takes effect after a restart.",
            local(c.applied).format("%b %-d")
        ),
        (Status::Applied, Some(c)) => format!("Applied {}.", local(c.applied).format("%b %-d")),
    }
}

/// What changed since it was applied: sizes then and now, growth before and since.
fn evidence(t: &Tweak, c: &Check) -> Vec<String> {
    let mut out = Vec::new();
    let then: Option<u64> = c.sizes.iter().map(|(_, a, _)| *a).sum();
    let now: Option<u64> = c.sizes.iter().map(|(_, _, b)| *b).sum();
    if let (Some(a), Some(b)) = (then, now)
        && !c.sizes.is_empty()
    {
        out.push(format!(
            "{}: {} then, {} now.",
            capitalized(t.what),
            bytes(a),
            bytes(b)
        ));
    }
    for tr in &c.trend {
        if let (Some(a), Some(b)) = (tr.before, tr.after) {
            out.push(format!(
                "{} grew {} a day in the week before, {} a day since.",
                printable(&tilde(&tr.owner)),
                signed_bytes(a),
                signed_bytes(b)
            ));
        }
    }
    out
}

fn signed_bytes(per_day: f64) -> String {
    if per_day < 0.0 {
        format!("-{}", bytes(per_day.abs() as u64))
    } else {
        bytes(per_day as u64)
    }
}

pub fn apply(db: &Path, id: &str, dry_run: bool, json_out: bool) -> Result<()> {
    let t = find(id)?;
    let home = home();
    if let Status::Absent(why) = t.status(&home)? {
        bail!("{id} doesn't apply to this Mac: {why}.");
    }
    let steps = if dry_run {
        t.plan(&home)?
    } else {
        let store = Store::open(db)?;
        t.apply(&home, &store, unix_now())?
    };
    let changes: Vec<String> = steps.iter().map(step_line).collect();
    // Profiles it installs, which nobody should have to take on trust.
    let profiles: Vec<(&Profile, &Path)> = steps
        .iter()
        .filter_map(|s| match &s.setting {
            Setting::Profile { profile, dir } if s.to.is_some() => Some((profile, dir.as_path())),
            _ => None,
        })
        .collect();
    let next = (!dry_run && !steps.is_empty())
        .then(|| next_line(t.after, profiles.first().map(|(p, _)| *p)));
    if json_out {
        let out = json!({
            "id": t.id,
            "title": t.title,
            "why": t.why,
            "cost": t.cost,
            "dry_run": dry_run,
            "changes": changes,
            "profiles": profiles
                .iter()
                .map(|(p, dir)| profile_json(p, dir))
                .collect::<Result<Vec<_>>>()?,
            "next": next,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!("{}", t.title);
    println!("  Why: {}", t.why);
    println!("  You give up: {}", t.cost);
    if changes.is_empty() {
        println!("Already applied: nothing to change.");
        return Ok(());
    }
    println!("  {}:", if dry_run { "Would change" } else { "Changed" });
    for c in &changes {
        println!("    {c}");
    }
    for (p, dir) in &profiles {
        println!("  The profile holds, for {} ({}):", p.app, p.domain);
        for (k, v) in &p.prefs {
            println!("    {k} = {v}");
        }
        println!("  System Settings shows with it: {}", p.description);
        if !dry_run {
            println!("  Saved as {}", tilde(&p.file_in(dir).to_string_lossy()));
        }
    }
    match next {
        None => println!("Nothing changed (dry run)."),
        Some(next) => {
            println!("{next}");
            println!(
                "Undo it with `mdif tweaks revert {id}`; `mdif tweaks` shows whether it worked."
            );
        }
    }
    Ok(())
}

/// What to expect once a tweak is applied.
fn next_line(after: After, profile: Option<&Profile>) -> String {
    match after {
        After::Nothing => "Done; it takes effect from now on.".into(),
        After::Relaunch(app) => format!("Done; it takes effect once {app} restarts."),
        After::Restart => "Done; restart the Mac when it suits you to finish.".into(),
        After::Approve(app) => format!(
            "Next: System Settings is open at {}. Double-click “{}”, read what it holds, then \
             click Install and enter your password. If macOS says it's unsigned or unverified, \
             that's because MyDiskIsFull made it on this Mac rather than a company signing it. \
             Then quit and reopen {app}. Until you install it, nothing changes.",
            profile::settings_place(),
            profile.map_or("the MyDiskIsFull profile", |p| p.name.as_str()),
        ),
    }
}

/// A profile as the app shows it before anyone installs it: what it holds,
/// what System Settings will say about it, and the file itself.
fn profile_json(p: &Profile, dir: &Path) -> Result<Value> {
    let settings: Vec<Value> = p
        .prefs
        .iter()
        .map(|(k, v)| json!({ "key": k, "value": v.to_string() }))
        .collect();
    Ok(json!({
        "name": p.name,
        "description": p.description,
        "app": p.app,
        "domain": p.domain,
        "settings": settings,
        "file": tilde(&p.file_in(dir).to_string_lossy()),
        "contents": String::from_utf8(p.mobileconfig()?)?,
        "place": profile::settings_place(),
    }))
}

pub fn revert(db: &Path, id: &str, json_out: bool) -> Result<()> {
    let t = find(id)?;
    if !db.exists() {
        bail!("MyDiskIsFull has no record of applying {id}, so it doesn't know what to put back.");
    }
    let store = Store::open(db)?;
    let r = t.revert(&store, unix_now())?;
    let restored: Vec<String> = r
        .restored
        .iter()
        .map(|s| {
            step_line(&Step {
                setting: s.setting.clone(),
                from: s.to.clone(),
                to: s.from.clone(),
            })
        })
        .collect();
    let kept: Vec<String> = r
        .kept
        .iter()
        .map(|(s, v)| format!("{}: {}", setting_name(s), value_text(s, v.as_ref())))
        .collect();
    let restart = t.after == After::Restart && !restored.is_empty();
    if json_out {
        let out = json!({
            "id": t.id,
            "title": t.title,
            "restored": restored,
            "kept": kept,
            "restart": restart,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if restored.is_empty() && kept.is_empty() {
        println!("Nothing to undo: {id}'s settings are already as they were.");
    }
    if !restored.is_empty() {
        println!("Put back:");
        for line in &restored {
            println!("    {line}");
        }
    }
    if !kept.is_empty() {
        println!("Left as they are (changed since the tweak):");
        for line in &kept {
            println!("    {line}");
        }
    }
    if restart {
        println!("It takes effect after a restart.");
    }
    Ok(())
}

/// `setting: before → after`.
fn step_line(s: &Step) -> String {
    format!(
        "{}: {} → {}",
        setting_name(&s.setting),
        value_text(&s.setting, s.from.as_ref()),
        value_text(&s.setting, s.to.as_ref())
    )
}

fn setting_name(s: &Setting) -> String {
    let text = match s {
        Setting::Json { file, key } | Setting::Env { file, key } => {
            format!("{}: {key}", tilde(&file.to_string_lossy()))
        }
        Setting::File { path } => tilde(&path.to_string_lossy()),
        other => other.to_string(),
    };
    printable(&text).into_owned()
}

fn value_text(s: &Setting, v: Option<&Value>) -> String {
    match (s, v) {
        (Setting::Profile { .. }, None) => "not installed".into(),
        (Setting::Profile { .. }, Some(_)) => "installed".into(),
        (Setting::File { .. }, None) => "absent".into(),
        (Setting::File { .. }, Some(Value::String(t))) => {
            format!("a file of {} lines", t.lines().count())
        }
        (_, None) => "not set".into(),
        (_, Some(v)) => printable(&v.to_string()).into_owned(),
    }
}

pub fn suggest(db: &Path, volume: &Path, json_out: bool) -> Result<()> {
    let home = home();
    let store = store(db)?;
    let keep = crate::keep_cmd::load(db)?;
    let list = suggest::gather(&home, volume, store.as_ref(), &keep, unix_now())?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }
    let st = mdif_macos::fs::statfs(volume)?;
    println!(
        "Free now: {} of {} on {}.",
        bytes(st.avail),
        bytes(st.total),
        st.mount_point
    );
    if store.is_none() {
        println!(
            "No history yet, so only what's on the disk now counts. `mdif agent install` \
             starts the watcher, which finds what keeps growing."
        );
    }
    println!();
    if list.is_empty() {
        println!(
            "Nothing to suggest: no runaway fillers, and nothing a tweak would stop is taking \
             much space."
        );
        return Ok(());
    }
    for (n, s) in list.iter().enumerate() {
        println!("{:>2}. {}", n + 1, s.cause);
        let label = match s.fix {
            Fix::Leave(_) => "Leave it",
            _ => "Fix",
        };
        println!("    {label}: {}", fix_line(&s.fix));
        if let Some(a) = &s.action {
            let paths: Vec<String> = a.paths.iter().map(|p| shell_quote(p)).collect();
            println!(
                "    Or `mdif clean {}`: {}, after checking again.",
                paths.join(" "),
                a.label.to_lowercase()
            );
        }
    }
    Ok(())
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A fix as printed: commands' paths already quoted, but control characters
/// in them escaped too.
pub fn fix_line(f: &Fix) -> String {
    printable(&f.line()).into_owned()
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;
    use std::collections::BTreeSet;

    /// The fixture the app's contract check decodes (app/Tests): a tweak that
    /// installs a profile, which CI, without Chrome, can't produce. Held here
    /// to the real serialization.
    const FIXTURE: &str = include_str!("../../../app/Tests/apply-profile.json");

    fn keys(v: &Value) -> BTreeSet<String> {
        v.as_object().unwrap().keys().cloned().collect()
    }

    #[test]
    fn the_apps_profile_fixture_has_the_shape_apply_prints() {
        let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
        let shown = &fixture["profiles"][0];
        let p = Profile {
            identifier: "io.github.lgibelli.mydiskisfull.test".into(),
            name: "test".into(),
            description: "test".into(),
            app: "Test".into(),
            domain: "io.github.lgibelli.mydiskisfull.test".into(),
            prefs: Map::from_iter([("k".to_string(), json!(1))]),
        };
        let real = profile_json(&p, Path::new("/nowhere")).unwrap();
        assert_eq!(keys(&real), keys(shown));
        assert_eq!(keys(&real["settings"][0]), keys(&shown["settings"][0]));
        assert_eq!(real["settings"][0]["value"], "1");
    }
}
