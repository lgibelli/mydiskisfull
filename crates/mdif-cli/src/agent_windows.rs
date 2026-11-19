//! `mdif agent` on Windows: the watcher starts at login through the user's
//! `Run` key, as `mdif-watch.exe`, the same program as `mdif watch --quiet
//! --log` with no console window. The key is per user and needs no
//! administrator; Task Manager's Startup apps can switch it off, and `status`
//! says so when that is what happened.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use mdif_core::os::{proc, startup};
use mdif_core::store::Store;
use mdif_core::units::bytes;

use crate::watch_cmd;

/// The `Run` value's name: what Task Manager's Startup apps lists.
pub const NAME: &str = "MyDiskIsFull Watcher";
/// What the running watcher holds ([`startup::claim`]).
pub const INSTANCE: &str = "MyDiskIsFullWatcher";

const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `mdif-watch.exe`, next to this program.
fn watcher_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe()?
        .parent()
        .context("the program has no folder")?
        .join("mdif-watch.exe");
    if !exe.is_file() {
        bail!("{} is missing: it comes with mdif.exe", exe.display());
    }
    Ok(exe)
}

/// The command line the `Run` key holds. The path is quoted whole, so a folder
/// with spaces is one program and not a program and arguments.
fn command_line(exe: &std::path::Path, interval: u64) -> String {
    format!("\"{}\" --interval {interval}", exe.display())
}

/// The running `mdif-watch` processes of this user whose program is `exe`.
fn running(exe: &std::path::Path) -> Vec<i32> {
    proc::own_processes()
        .into_iter()
        .filter(|p| p.name.eq_ignore_ascii_case("mdif-watch"))
        .filter(|p| {
            proc::path(p.pid).is_some_and(|path| {
                path.to_string_lossy()
                    .eq_ignore_ascii_case(&exe.to_string_lossy())
            })
        })
        .map(|p| p.pid)
        .collect()
}

pub fn install(interval: u64) -> Result<()> {
    let exe = watcher_exe()?;
    std::fs::create_dir_all(watch_cmd::log_path().parent().unwrap())?;
    startup::add_login_item(NAME, &command_line(&exe, interval)).context("writing the Run key")?;
    if startup::is_claimed(INSTANCE) {
        println!(
            "The watcher is already running; the new settings apply from the next sign-in \
             (`mdif agent uninstall` stops it now)."
        );
    } else {
        Command::new(&exe)
            .args(["--interval", &interval.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .spawn()
            .with_context(|| format!("starting {}", exe.display()))?;
        println!(
            "Watcher installed and started: every {interval} s, history in {}.",
            watch_cmd::default_db().display()
        );
    }
    println!(
        "It starts when you sign in, as {}; if you move or delete that program, run `mdif agent install` again.",
        exe.display()
    );
    println!("Check on it with `mdif agent status`, and see the forecast with `mdif forecast`.");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let exe = watcher_exe();
    let had = startup::remove_login_item(NAME).context("removing the Run entry")?;
    // Only the installed program itself, found by where it lives, not by name.
    let stopped = match &exe {
        Ok(exe) => running(exe)
            .into_iter()
            .filter(|&pid| proc::force_quit(pid).is_ok())
            .count(),
        Err(_) => 0,
    };
    println!(
        "{} {} The history stays in {}.",
        if had {
            "No longer starts at sign-in."
        } else {
            "It wasn't set to start at sign-in."
        },
        if stopped > 0 {
            "Watcher stopped."
        } else {
            "No watcher was running."
        },
        watch_cmd::default_db().display()
    );
    Ok(())
}

pub fn status() -> Result<()> {
    match startup::login_item(NAME)? {
        Some(item) if item.enabled => println!("Starts at sign-in: yes ({})", item.command),
        Some(item) => println!(
            "Starts at sign-in: switched off in Task Manager's Startup apps ({})",
            item.command
        ),
        None => println!("Starts at sign-in: no (`mdif agent install`)"),
    }
    if startup::is_claimed(INSTANCE) {
        println!("Watcher: running");
    } else {
        println!("Watcher: not running (`mdif agent install`, or `mdif watch`)");
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
    fn the_command_line_quotes_the_program_whole() {
        let line = command_line(
            std::path::Path::new(r"C:\Program Files\x\mdif-watch.exe"),
            300,
        );
        assert_eq!(
            line,
            r#""C:\Program Files\x\mdif-watch.exe" --interval 300"#
        );
    }
}
