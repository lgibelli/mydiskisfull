//! `mdif agent`: run the watcher in the background as a LaunchAgent.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use mdif_core::store::Store;
use mdif_core::units::bytes;

use crate::watch_cmd;

pub const LABEL: &str = "io.github.lgibelli.mydiskisfull.watch";

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from)
}

fn plist_path() -> PathBuf {
    home()
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

fn log_path() -> PathBuf {
    home().join("Library/Logs/MyDiskIsFull/watch.log")
}

fn domain() -> Result<String> {
    let out = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .context("running id -u")?;
    Ok(format!(
        "gui/{}",
        String::from_utf8_lossy(&out.stdout).trim()
    ))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn install(interval: u64) -> Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let exe = exe.to_str().context("executable path is not UTF-8")?;
    let log = log_path();
    std::fs::create_dir_all(log.parent().unwrap())?;
    let plist = plist_path();
    std::fs::create_dir_all(plist.parent().unwrap())?;
    // --quiet: only errors reach the log, so the log can't grow by itself.
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>watch</string>
    <string>--quiet</string>
    <string>--interval</string>
    <string>{interval}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ProcessType</key>
  <string>Background</string>
  <key>LowPriorityIO</key>
  <true/>
  <key>Nice</key>
  <integer>10</integer>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
        exe = xml_escape(exe),
        log = xml_escape(&log.to_string_lossy()),
    );
    std::fs::write(&plist, body)?;

    let domain = domain()?;
    // Replace a previous installation, if any.
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{domain}/{LABEL}")])
        .output();
    let out = Command::new("launchctl")
        .args(["bootstrap", &domain])
        .arg(&plist)
        .output()
        .context("running launchctl")?;
    if !out.status.success() {
        bail!(
            "launchctl bootstrap failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    println!(
        "Watcher installed and running: every {interval} s, history in {}.",
        watch_cmd::default_db().display()
    );
    println!("It runs {exe}; if you move or delete that binary, run `mdif agent install` again.");
    println!("Check on it with `mdif agent status`, and see the forecast with `mdif forecast`.");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let domain = domain()?;
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{domain}/{LABEL}")])
        .output();
    let plist = plist_path();
    if plist.exists() {
        std::fs::remove_file(&plist)?;
    }
    println!(
        "Watcher stopped and removed. The history stays in {}.",
        watch_cmd::default_db().display()
    );
    Ok(())
}

pub fn status() -> Result<()> {
    let domain = domain()?;
    let out = Command::new("launchctl")
        .args(["print", &format!("{domain}/{LABEL}")])
        .output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        println!("Watcher: not installed (run `mdif agent install`).");
    } else {
        let field = |k: &str| {
            text.lines().map(str::trim).find_map(|l| {
                l.strip_prefix(k)
                    .map(|v| v.trim_start_matches([' ', '=']).trim().to_string())
            })
        };
        println!(
            "Watcher: {} (pid {})",
            field("state").unwrap_or_else(|| "unknown".into()),
            field("pid").unwrap_or_else(|| "-".into())
        );
    }
    let db = watch_cmd::default_db();
    if db.exists() {
        let size: u64 = ["", "-wal", "-shm"]
            .iter()
            .filter_map(|s| std::fs::metadata(format!("{}{s}", db.display())).ok())
            .map(|m| m.len())
            .sum();
        let store = Store::open(&db)?;
        let rows: i64 = store.row_counts()?.iter().map(|(_, _, n)| n).sum();
        println!("History: {} ({rows} rows) in {}", bytes(size), db.display());
    } else {
        println!("History: none yet");
    }
    Ok(())
}
