//! `mdif agent`: run the watcher in the background as a LaunchAgent.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use mdif_core::store::Store;
use mdif_core::units::bytes;

use crate::watch_cmd;

pub const LABEL: &str = "io.github.lgibelli.mydiskisfull.watch";
/// The menu bar app's watcher (an SMAppService agent). Not [`LABEL`]: macOS
/// keeps a record of a command-line agent under its label, and an app's
/// agent with the same label inherits it and is refused once it's disabled.
pub const APP_LABEL: &str = "io.github.lgibelli.mydiskisfull.watcher";

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from)
}

fn plist_path() -> PathBuf {
    home()
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
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

/// The app bundle whose LaunchAgent this is, when the app runs the watcher.
fn run_by_app(launchctl_print: &str) -> Option<String> {
    launchctl_print.lines().find_map(|l| {
        let path = l.trim().strip_prefix("program = ")?;
        let end = path.find(".app/Contents/")? + ".app".len();
        Some(path[..end].to_string())
    })
}

/// What launchd says about the job `label`, if it has one.
fn print_job(domain: &str, label: &str) -> Option<String> {
    let out = Command::new("launchctl")
        .args(["print", &format!("{domain}/{label}")])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Two watchers would sample the same history twice.
fn refuse_if_app(domain: &str) -> Result<()> {
    if let Some(text) = print_job(domain, APP_LABEL) {
        let app = run_by_app(&text).unwrap_or_else(|| "MyDiskIsFull".into());
        bail!(
            "the watcher already runs in {app}, with the same history. Turn \"Watch in the \
             background\" off in its settings first if you'd rather run it from here."
        );
    }
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn install(interval: u64) -> Result<()> {
    let domain = domain()?;
    refuse_if_app(&domain)?;
    let exe = std::env::current_exe()?.canonicalize()?;
    let exe = exe.to_str().context("executable path is not UTF-8")?;
    let log = watch_cmd::log_path();
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
    let mut found = false;
    for (label, by) in [(LABEL, "mdif agent install"), (APP_LABEL, "the app")] {
        let Some(text) = print_job(&domain, label) else {
            continue;
        };
        found = true;
        let field = |k: &str| {
            text.lines().map(str::trim).find_map(|l| {
                l.strip_prefix(k)
                    .map(|v| v.trim_start_matches([' ', '=']).trim().to_string())
            })
        };
        println!(
            "Watcher: {} (pid {}), run by {}",
            field("state").unwrap_or_else(|| "unknown".into()),
            field("pid").unwrap_or_else(|| "-".into()),
            run_by_app(&text).unwrap_or_else(|| by.into())
        );
    }
    if !found {
        println!(
            "Watcher: not running (`mdif agent install`, or \"Watch in the background\" in the app)."
        );
    }
    let db = watch_cmd::default_db();
    if db.exists() {
        let size = watch_cmd::history_bytes(&db);
        let store = Store::open(&db)?;
        let rows: i64 = store.row_counts()?.iter().map(|(_, _, n)| n).sum();
        println!("History: {} ({rows} rows) in {}", bytes(size), db.display());
    } else {
        println!("History: none yet");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_is_found_from_the_program_path() {
        let app = "\tprogram = /Applications/MyDiskIsFull.app/Contents/MacOS/mdif\n";
        assert_eq!(
            run_by_app(app).as_deref(),
            Some("/Applications/MyDiskIsFull.app")
        );
        let cli = "\tprogram = /Users/me/.cargo/bin/mdif\n";
        assert_eq!(run_by_app(cli), None);
    }
}
