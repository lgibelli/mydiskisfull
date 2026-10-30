//! `mdif knowledge`: what's known about folders (see mdif_core::knowledge),
//! `update` for a newer list, `about <folder>` for what's said of one,
//! `missing` for this Mac's folders nothing in particular is said about,
//! `ask <folder>` to ask AI about one (mdif_core::ask), and `check <file>`
//! for a list before it's put online.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mdif_core::knowledge::{self, Known};
use mdif_core::text::printable;
use mdif_core::units::bytes;
use mdif_core::watch::unix_now;
use serde_json::json;

use crate::watch_cmd::home;

/// Where the downloaded list is: beside the history.
pub fn dir() -> PathBuf {
    crate::watch_cmd::default_db()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

pub fn show(json_out: bool) -> Result<()> {
    let l = knowledge::loaded();
    let k = &l.knowledge;
    if json_out {
        let s = json!({
            "version": k.version,
            "folders": k.folders.len(),
            "downloaded": l.downloaded,
            "url": knowledge::URL,
        });
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }
    println!(
        "Knows about {} folders: version {}, {}.",
        k.folders.len(),
        k.version,
        if l.downloaded {
            "downloaded"
        } else {
            "built in"
        }
    );
    println!(
        "`mdif knowledge update` downloads a newer list when there is one, signed like the \
         app's updates; nothing about this Mac is sent."
    );
    Ok(())
}

pub fn update(json_out: bool) -> Result<()> {
    let u = knowledge::update(&dir()).context("couldn't look for a newer list")?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&u)?);
    } else if u.newer {
        println!(
            "A newer list: version {}, {} folders.",
            u.version, u.folders
        );
    } else {
        println!("No list newer than version {}.", u.version);
    }
    Ok(())
}

struct About {
    path: String,
    name: Option<String>,
    note: Option<String>,
    known: Option<Known>,
}

pub fn about(paths: &[PathBuf], json_out: bool) -> Result<()> {
    let home = home();
    let found: Vec<About> = paths
        .iter()
        .map(|p| {
            let p = std::path::absolute(p).unwrap_or_else(|_| p.clone());
            About {
                name: knowledge::called(&p, &home),
                note: knowledge::note(&p, &home),
                known: knowledge::known(&p, &home),
                path: p.to_string_lossy().into_owned(),
            }
        })
        .collect();
    if json_out {
        let found: Vec<_> = found
            .iter()
            .map(|a| json!({"path": a.path, "name": a.name, "note": a.note, "known": a.known}))
            .collect();
        println!("{}", serde_json::to_string_pretty(&found)?);
        return Ok(());
    }
    for a in &found {
        let shown = a.path.replacen(&*home.to_string_lossy(), "~", 1);
        match &a.name {
            Some(n) => println!("{}: {}", printable(n), printable(&shown)),
            None => println!("{}", printable(&shown)),
        }
        let k = a.known.as_ref();
        let lines = [
            ("", a.note.as_ref()),
            ("", k.and_then(|k| k.what.as_ref())),
            ("Why it grows: ", k.and_then(|k| k.grows.as_ref())),
            ("What helps: ", k.and_then(|k| k.helps.as_ref())),
            ("Source: ", k.and_then(|k| k.source.as_ref())),
        ];
        let said: Vec<_> = lines
            .into_iter()
            .filter_map(|(label, t)| t.map(|t| format!("  {label}{}", printable(t))))
            .collect();
        if said.is_empty() {
            println!("  Nothing known about it.");
        }
        for l in said {
            println!("{l}");
        }
    }
    Ok(())
}

/// A folder nothing in particular is said about.
struct Missing {
    /// As it may be asked about (knowledge::generic).
    name: String,
    path: String,
    bytes: u64,
    /// In the last week, as the watcher saw it.
    grew: i64,
}

/// This Mac's folders, `min_mb` or more or grown as much in a week, that
/// nothing in particular is said about: from the last measurement (`mdif
/// space`) and the watcher's history. Only names that say nothing about
/// you: never your own folders.
pub fn missing(min_mb: u64, json_out: bool) -> Result<()> {
    let db = crate::watch_cmd::default_db();
    let home = home();
    let min = min_mb * 1_000_000;
    let mut found: Vec<Missing> = Vec::new();
    let mut add = |name: String, path: &str, size: u64, grew: i64| match found
        .iter_mut()
        .find(|m| m.name == name)
    {
        Some(m) => {
            m.bytes = m.bytes.max(size);
            m.grew = m.grew.max(grew);
        }
        None => found.push(Missing {
            name,
            path: path.to_string(),
            bytes: size,
            grew,
        }),
    };
    let measured = crate::space_cmd::load(&crate::space_cmd::file(&db));
    if let Some(m) = &measured {
        let keep = crate::keep_cmd::load(&db)?;
        let r = mdif_core::space::report(m, &keep, &[]);
        for i in r.kinds.iter().flat_map(|k| &k.items) {
            if let Some(n) = &i.unexplained
                && i.bytes >= min
            {
                add(n.clone(), &i.path, i.bytes, 0);
            }
        }
    }
    if db.exists()
        && let Ok(store) = mdif_core::store::Store::open(&db)
    {
        for (path, grew) in store
            .growth_since(unix_now() - 7 * 86_400)
            .unwrap_or_default()
        {
            let p = Path::new(&path);
            // An app grows by its updates; macOS's temporary files are
            // said as a kind.
            let ruled =
                p.extension().is_some_and(|e| e == "app") || p.starts_with("/private/var/folders");
            if !ruled
                && grew >= min as i64
                && knowledge::known(p, &home).is_none()
                && knowledge::note(p, &home).is_none()
                && let Some(n) = knowledge::generic(p, &home)
            {
                add(n, &path, 0, grew);
            }
        }
    }
    found.sort_by_key(|m| std::cmp::Reverse(m.bytes.max(m.grew.max(0) as u64)));
    if json_out {
        let found: Vec<_> = found
            .iter()
            .map(|m| json!({"name": m.name, "path": m.path, "bytes": m.bytes, "grew": m.grew}))
            .collect();
        println!("{}", serde_json::to_string_pretty(&found)?);
        return Ok(());
    }
    if measured.is_none() {
        println!(
            "Not measured yet: `mdif space` measures the disk (about 30 s). From the history alone:"
        );
    }
    if found.is_empty() {
        println!("Something is said about every folder of {min_mb} MB or more on this Mac.");
        return Ok(());
    }
    println!(
        "Nothing in particular is known about {} folder{} on this Mac ({} or more, or grown as \
         much in a week):",
        found.len(),
        if found.len() == 1 { "" } else { "s" },
        bytes(min)
    );
    for m in &found {
        let size = if m.bytes > 0 {
            bytes(m.bytes)
        } else {
            String::new()
        };
        let grew = if m.grew > 0 {
            format!("+{} this week", bytes(m.grew as u64))
        } else {
            String::new()
        };
        println!("  {size:>9}  {grew:<17} {}", printable(&m.name));
    }
    println!(
        "Each is named as it would be asked about: nothing in it, and none of your own folders. \
         What's known goes in crates/mdif-core/knowledge/folders.json."
    );
    Ok(())
}

pub fn ask(folder: &Path, size: Option<u64>, dry_run: bool, json_out: bool) -> Result<()> {
    let p = std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf());
    let q = mdif_core::ask::question(&p, &home(), size).map_err(|e| {
        anyhow::anyhow!(
            "can't ask about {}: {e}",
            crate::watch_cmd::tilde(&p.to_string_lossy())
        )
    })?;
    if dry_run {
        if json_out {
            let s = json!({"name": q.name, "bytes": q.bytes, "macos": q.macos, "url": mdif_core::ask::URL});
            println!("{}", serde_json::to_string_pretty(&s)?);
        } else {
            println!("Would send to {}:", mdif_core::ask::URL);
            println!("  {}", printable(&q.name));
            if let Some(b) = q.bytes {
                println!("  {}", bytes(b));
            }
            println!("  macOS {}", q.macos);
            println!("Nothing else: nothing inside the folder, nothing about you.");
        }
        return Ok(());
    }
    let dir = dir();
    let now = unix_now();
    let a = match mdif_core::ask::kept(&dir, &q.name, now) {
        Some(a) => a,
        None => mdif_core::ask::ask(&dir, &q, now)?,
    };
    if json_out {
        println!("{}", serde_json::to_string_pretty(&a)?);
        return Ok(());
    }
    println!("{}", printable(&a.name));
    if !a.known {
        println!("  The AI doesn't know what it is either.");
        return Ok(());
    }
    for (label, t) in [
        ("", &a.what),
        ("Why it grows: ", &a.grows),
        ("What helps: ", &a.helps),
    ] {
        if let Some(t) = t {
            println!("  {label}{}", printable(t));
        }
    }
    println!("Written by AI ({}) and not checked: it may be wrong.", a.by);
    Ok(())
}

pub fn check(file: &Path, sig: Option<&Path>) -> Result<()> {
    let bytes = std::fs::read(file).with_context(|| format!("{}", file.display()))?;
    let sig = sig
        .map(|s| std::fs::read_to_string(s).with_context(|| format!("{}", s.display())))
        .transpose()?;
    let k = knowledge::check(&bytes, sig.as_deref())
        .with_context(|| format!("{} isn't a list to put online", file.display()))?;
    println!(
        "Version {}, {} folders{}.",
        k.version,
        k.folders.len(),
        if sig.is_some() {
            ", signed with the updates' key"
        } else {
            ""
        }
    );
    Ok(())
}
